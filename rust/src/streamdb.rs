// StreamTune - Lightweight Music Player/Manager
// Copyright (C) 2025 DeMoD LLC
//
// This library is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this library. If not, see <https://www.gnu.org/licenses/>.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use parking_lot::{Mutex, RwLock as PlRwLock};
use memmap2::{MmapMut, MmapOptions};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use uuid::Uuid;
use crc::Crc as CrcLib;
use crc::CRC_32_ISO_HDLC;
use lru::LruCache;
use snappy;
use futures::future::ready;
use futures::Future;
use tracing::{info, error};

const MAGIC: [u8; 8] = [0x55, 0xAA, 0xFE, 0xED, 0xFA, 0xCE, 0xDA, 0x7A];
const DEFAULT_PAGE_RAW_SIZE: u64 = 8192; // 8KB for NVMe
const DEFAULT_PAGE_HEADER_SIZE: u64 = 32; // crc(4)+ver(4)+prev/next(8+8)+flags(1)+len(4)+pad(3)
const FREE_LIST_HEADER_SIZE: u64 = 12; // next(8)+used(4)
const FREE_LIST_ENTRIES_PER_PAGE: usize = ((DEFAULT_PAGE_RAW_SIZE - DEFAULT_PAGE_HEADER_SIZE - FREE_LIST_HEADER_SIZE) / 8) as usize; // 1011
const DEFAULT_MAX_DB_SIZE: u64 = 8000 * 1024 * 1024 * 1024; // 8TB
const DEFAULT_MAX_PAGES: i64 = i64::MAX;
const DEFAULT_MAX_DOCUMENT_SIZE: u64 = 256 * 1024 * 1024; // 256MB
const BATCH_GROW_PAGES: u64 = 16;
const DEFAULT_PAGE_CACHE_SIZE: usize = 2048;
const DEFAULT_VERSIONS_TO_KEEP: i32 = 2;
const MAX_CONSECUTIVE_EMPTY_FREE_LIST: u64 = 5;

const FLAG_DATA_PAGE: u8 = 0b00000001;
const FLAG_TRIE_PAGE: u8 = 0b00000010;
const FLAG_FREE_LIST_PAGE: u8 = 0b00000100;
const FLAG_INDEX_PAGE: u8 = 0b00001000;

#[derive(thiserror::Error, Debug)]
pub enum StreamDBError {
    #[error("IO error: {0}")]
    IOError(String),
    #[error("Invalid key: {0}")]
    InvalidKey(String),
    #[error("Version conflict: {0}")]
    VersionConflict(String),
    #[error("Chunking error: {0}")]
    ChunkError(String),
}

#[derive(Clone, Copy, Debug)]
struct VersionedLink {
    page_id: i64,
    version: i32,
}

#[derive(Debug)]
struct DatabaseHeader {
    magic: [u8; 8],
    index_root: VersionedLink,
    path_lookup_root: VersionedLink,
    free_list_root: VersionedLink,
}

#[derive(Debug, Clone)]
struct PageHeader {
    crc: u32,
    version: i32,
    prev_page_id: i64,
    next_page_id: i64,
    flags: u8,
    data_length: i32,
    padding: [u8; 3],
}

#[derive(Debug)]
struct FreeListPage {
    next_free_list_page: i64,
    used_entries: i32,
    free_page_ids: Vec<i64>,
}

#[derive(Debug)]
struct ReverseTrieNode {
    edge: String,
    parent_index: i64,
    self_index: i64,
    document_id: Option<Uuid>,
    children: HashMap<char, i64>,
}

#[derive(Clone, Debug)]
struct Document {
    id: Uuid,
    first_page_id: i64,
    current_version: i32,
    paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    page_size: u64,
    page_header_size: u64,
    max_db_size: u64,
    max_pages: i64,
    max_document_size: u64,
    page_cache_size: usize,
    versions_to_keep: i32,
    use_mmap: bool,
    use_compression: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_RAW_SIZE,
            page_header_size: DEFAULT_PAGE_HEADER_SIZE,
            max_db_size: DEFAULT_MAX_DB_SIZE,
            max_pages: DEFAULT_MAX_PAGES,
            max_document_size: DEFAULT_MAX_DOCUMENT_SIZE,
            page_cache_size: DEFAULT_PAGE_CACHE_SIZE,
            versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
            use_mmap: true,
            use_compression: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct CacheStats {
    hits: usize,
    misses: usize,
}

pub trait Database {
    fn insert(&mut self, key: &[u8], value: &[u8], version: Option<u64>) -> Result<(), StreamDBError>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StreamDBError>;
    fn get_quick(&self, key: &[u8], quick: bool) -> Result<Option<Vec<u8>>, StreamDBError>;
    fn get_id_by_path(&self, path: &str) -> Result<Option<Uuid>, StreamDBError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), StreamDBError>;
    fn delete_by_id(&mut self, id: Uuid) -> Result<(), StreamDBError>;
    fn bind_to_path(&mut self, id: Uuid, path: &str) -> Result<(), StreamDBError>;
    fn unbind_path(&mut self, id: Uuid, path: &str) -> Result<(), StreamDBError>;
    fn prefix_search(&self, prefix: &[u8]) -> Result<Vec<Vec<u8>>, StreamDBError>;
    fn list_paths(&self, id: Uuid) -> Result<Vec<String>, StreamDBError>;
    fn flush(&self) -> Result<(), StreamDBError>;
    fn calculate_statistics(&self) -> Result<(i64, i64), StreamDBError>;
    fn set_quick_mode(&mut self, enabled: bool);
    fn snapshot(&self) -> Result<Self, StreamDBError> where Self: Sized;
    fn get_cache_stats(&self) -> Result<CacheStats, StreamDBError>;
    fn get_stream(&self, key: &[u8]) -> Result<Box<dyn Iterator<Item = Result<Vec<u8>, StreamDBError>> + Send>, StreamDBError>;
    fn get_async(&self, key: &[u8]) -> Box<dyn Future<Output = Result<Option<Vec<u8>>, StreamDBError>> + Send + Unpin>;
}

pub trait DatabaseBackend {
    fn write_document(&mut self, data: &mut dyn Read) -> Result<Uuid, StreamDBError>;
    fn read_document(&self, id: Uuid) -> Result<Vec<u8>, StreamDBError>;
    fn read_document_quick(&self, id: Uuid, quick: bool) -> Result<Vec<u8>, StreamDBError>;
    fn delete_document(&mut self, id: Uuid) -> Result<(), StreamDBError>;
    fn bind_path_to_document(&mut self, path: &str, id: Uuid) -> Result<Uuid, StreamDBError>;
    fn get_document_id_by_path(&self, path: &str) -> Result<Uuid, StreamDBError>;
    fn search_paths(&self, prefix: &str) -> Result<Vec<String>, StreamDBError>;
    fn list_paths_for_document(&self, id: Uuid) -> Result<Vec<String>, StreamDBError>;
    fn count_free_pages(&self) -> Result<i64, StreamDBError>;
    fn get_info(&self, id: Uuid) -> Result<String, StreamDBError>;
    fn delete_paths_for_document(&mut self, id: Uuid) -> Result<(), StreamDBError>;
    fn remove_from_index(&mut self, id: Uuid) -> Result<(), StreamDBError>;
    fn get_cache_stats(&self) -> Result<CacheStats, StreamDBError>;
    fn get_stream(&self, id: Uuid) -> Result<Box<dyn Iterator<Item = Result<Vec<u8>, StreamDBError>> + Send>, StreamDBError>;
    fn flush(&self) -> Result<(), StreamDBError>;
}

fn write_varint<W: Write>(writer: &mut W, mut value: u64) -> Result<(), StreamDBError> {
    loop {
        if value < 0x80 {
            writer.write_u8(value as u8)
                .map_err(|e| StreamDBError::IOError(format!("Failed to write varint: {}", e)))?;
            break;
        }
        writer.write_u8((value as u8 & 0x7F) | 0x80)
            .map_err(|e| StreamDBError::IOError(format!("Failed to write varint: {}", e)))?;
        value >>= 7;
    }
    Ok(())
}

fn read_varint<R: Read>(reader: &mut R) -> Result<u64, StreamDBError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = reader.read_u8()
            .map_err(|e| StreamDBError::IOError(format!("Failed to read varint: {}", e)))?;
        value |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
        if shift > 63 {
            return Err(StreamDBError::IOError("Varint too large".to_string()));
        }
    }
    Ok(value)
}

fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

struct MemoryBackend {
    config: Config,
    documents: Mutex<HashMap<Uuid, Vec<u8>>>,
    path_to_id: Mutex<HashMap<String, Uuid>>,
    id_to_paths: Mutex<HashMap<Uuid, Vec<String>>>,
    trie_root: Mutex<ReverseTrieNode>,
    cache_stats: Mutex<CacheStats>,
}

impl MemoryBackend {
    fn new(config: Config) -> Self {
        Self {
            config,
            documents: Mutex::new(HashMap::new()),
            path_to_id: Mutex::new(HashMap::new()),
            id_to_paths: Mutex::new(HashMap::new()),
            trie_root: Mutex::new(ReverseTrieNode {
                edge: String::new(),
                parent_index: -1,
                self_index: 0,
                document_id: None,
                children: HashMap::new(),
            }),
            cache_stats: Mutex::new(CacheStats::default()),
        }
    }

    fn validate_path(&self, path: &str) -> Result<(), StreamDBError> {
        if path.is_empty() || path.contains('\0') || path.contains("//") {
            return Err(StreamDBError::InvalidKey("Invalid path".to_string()));
        }
        if path.len() > 1024 {
            return Err(StreamDBError::InvalidKey("Path too long".to_string()));
        }
        Ok(())
    }

    fn serialize_trie_node(&self, node: &ReverseTrieNode) -> Result<Vec<u8>, StreamDBError> {
        let edge_bytes = node.edge.as_bytes();
        let estimated_size = 5 + edge_bytes.len() + 10 + 10 + 1 + 17 + 5 + node.children.len() * (5 + 10);
        if estimated_size as u64 > self.config.page_size - self.config.page_header_size {
            return Err(StreamDBError::InvalidInput("Trie node too large".to_string()));
        }
        let mut buffer = Vec::new();
        let mut writer = BufWriter::new(&mut buffer);
        write_varint(&mut writer, edge_bytes.len() as u64)?;
        writer.write_all(edge_bytes)
            .map_err(|e| StreamDBError::IOError(format!("Failed to write edge: {}", e)))?;
        write_varint(&mut writer, zigzag_encode(node.parent_index))?;
        write_varint(&mut writer, zigzag_encode(node.self_index))?;
        match node.document_id {
            Some(id) => {
                writer.write_u8(1)
                    .map_err(|e| StreamDBError::IOError(format!("Failed to write id flag: {}", e)))?;
                writer.write_all(id.as_bytes())
                    .map_err(|e| StreamDBError::IOError(format!("Failed to write id: {}", e)))?;
            }
            None => writer.write_u8(0)
                .map_err(|e| StreamDBError::IOError(format!("Failed to write id flag: {}", e)))?,
        }
        write_varint(&mut writer, node.children.len() as u64)?;
        for (&c, &index) in &node.children {
            write_varint(&mut writer, c as u64)?;
            write_varint(&mut writer, zigzag_encode(index))?;
        }
        Ok(buffer)
    }

    fn deserialize_trie_node(&self, data: &[u8]) -> Result<ReverseTrieNode, StreamDBError> {
        let mut reader = Cursor::new(data);
        let edge_len = read_varint(&mut reader)? as usize;
        let mut edge_bytes = vec![0u8; edge_len];
        reader.read_exact(&mut edge_bytes)
            .map_err(|e| StreamDBError::IOError(format!("Failed to read edge: {}", e)))?;
        let edge = String::from_utf8(edge_bytes)
            .map_err(|e| StreamDBError::IOError(format!("Invalid UTF-8 edge: {}", e)))?;
        let parent_index = zigzag_decode(read_varint(&mut reader)?);
        let self_index = zigzag_decode(read_varint(&mut reader)?);
        let has_id = reader.read_u8()
            .map_err(|e| StreamDBError::IOError(format!("Failed to read id flag: {}", e)))?;
        let document_id = if has_id == 1 {
            let mut bytes = [0u8; 16];
            reader.read_exact(&mut bytes)
                .map_err(|e| StreamDBError::IOError(format!("Failed to read id: {}", e)))?;
            Some(Uuid::from_bytes(bytes))
        } else {
            None
        };
        let children_count = read_varint(&mut reader)? as usize;
        let mut children = HashMap::with_capacity(children_count);
        for _ in 0..children_count {
            let c = read_varint(&mut reader)?;
            let c_char = char::try_from(c as u32)
                .map_err(|_| StreamDBError::IOError("Invalid child char".to_string()))?;
            let index = zigzag_decode(read_varint(&mut reader)?);
            children.insert(c_char, index);
        }
        Ok(ReverseTrieNode {
            edge,
            parent_index,
            self_index,
            document_id,
            children,
        })
    }

    fn trie_insert(&self, path: &str, id: Uuid) -> Result<(), StreamDBError> {
        self.validate_path(path)?;
        let reversed: String = path.chars().rev().collect();
        let mut current = self.trie_root.lock();
        let mut remaining = reversed.as_str();
        let mut path_stack = vec![];
        while !remaining.is_empty() {
            let first_char = remaining.chars().next().unwrap();
            if let Some(&child_index) = current.children.get(&first_char) {
                let child = self.deserialize_trie_node(&self.serialize_trie_node(&current)?)?; // Simulate page read
                if child.self_index == child_index {
                    let edge = child.edge.as_str();
                    let common_len = edge.chars().zip(remaining.chars()).take_while(|(a, b)| a == b).count();
                    if common_len == edge.len() {
                        path_stack.push((current.self_index, current.clone()));
                        *current = child;
                        remaining = &remaining[common_len..];
                        continue;
                    } else if common_len > 0 {
                        let common = &edge[..common_len];
                        let suffix = &edge[common_len..];
                        let new_intermediate_index = child.self_index + 1;
                        let new_child_index = new_intermediate_index + 1;
                        let new_intermediate = ReverseTrieNode {
                            edge: common.to_string(),
                            parent_index: current.self_index,
                            self_index: new_intermediate_index,
                            document_id: None,
                            children: HashMap::from([(suffix.chars().next().unwrap(), new_child_index)]),
                        };
                        let new_child = ReverseTrieNode {
                            edge: suffix.to_string(),
                            parent_index: new_intermediate_index,
                            self_index: new_child_index,
                            document_id: child.document_id,
                            children: child.children,
                        };
                        current.children.insert(first_char, new_intermediate_index);
                        let mut trie_root = self.trie_root.lock();
                        trie_root.children.insert(first_char, new_intermediate_index);
                        path_stack.push((current.self_index, current.clone()));
                        *current = new_intermediate;
                        trie_root.children.insert(suffix.chars().next().unwrap(), new_child_index);
                        *trie_root = new_child;
                        remaining = &remaining[common_len..];
                    } else {
                        let new_child_index = current.self_index + 1;
                        let new_child = ReverseTrieNode {
                            edge: remaining.to_string(),
                            parent_index: current.self_index,
                            self_index: new_child_index,
                            document_id: Some(id),
                            children: HashMap::new(),
                        };
                        current.children.insert(first_char, new_child_index);
                        let mut trie_root = self.trie_root.lock();
                        *trie_root = new_child;
                        return Ok(());
                    }
                } else {
                    return Err(StreamDBError::InvalidKey("Invalid trie index".to_string()));
                }
            } else {
                let new_child_index = current.self_index + 1;
                let new_child = ReverseTrieNode {
                    edge: remaining.to_string(),
                    parent_index: current.self_index,
                    self_index: new_child_index,
                    document_id: Some(id),
                    children: HashMap::new(),
                };
                current.children.insert(first_char, new_child_index);
                let mut trie_root = self.trie_root.lock();
                *trie_root = new_child;
                return Ok(());
            }
        }
        current.document_id = Some(id);
        Ok(())
    }

    fn trie_delete(&self, path: &str) -> Result<(), StreamDBError> {
        self.validate_path(path)?;
        let reversed: String = path.chars().rev().collect();
        let mut current = self.trie_root.lock();
        let mut remaining = reversed.as_str();
        let mut path_stack = vec![];
        while !remaining.is_empty() {
            let first_char = remaining.chars().next().unwrap();
            if let Some(&child_index) = current.children.get(&first_char) {
                let child = self.deserialize_trie_node(&self.serialize_trie_node(&current)?)?; // Simulate page read
                if child.self_index == child_index {
                    let edge = child.edge.as_str();
                    if remaining.starts_with(edge) {
                        path_stack.push((current.self_index, current.clone(), first_char));
                        *current = child;
                        remaining = &remaining[edge.len()..];
                    } else {
                        return Err(StreamDBError::InvalidKey("Path not found".to_string()));
                    }
                } else {
                    return Err(StreamDBError::InvalidKey("Path not found".to_string()));
                }
            } else {
                return Err(StreamDBError::InvalidKey("Path not found".to_string()));
            }
        }
        if current.document_id.is_none() {
            return Err(StreamDBError::InvalidKey("Path not found".to_string()));
        }
        current.document_id = None;
        while let Some((parent_index, parent_node, c)) = path_stack.pop() {
            let mut parent = self.trie_root.lock();
            if parent.self_index == parent_index {
                if current.document_id.is_none() && current.children.len() == 1 {
                    let (child_char, child_index) = current.children.iter().next().unwrap().clone();
                    let child_node = self.deserialize_trie_node(&self.serialize_trie_node(&current)?)?;
                    if child_node.self_index == child_index {
                        let merged_edge = format!("{}{}", current.edge, child_node.edge);
                        let merged_node = ReverseTrieNode {
                            edge: merged_edge,
                            parent_index: parent.self_index,
                            self_index: current.self_index,
                            document_id: child_node.document_id,
                            children: child_node.children,
                        };
                        *current = merged_node;
                        parent.children.insert(c, current.self_index);
                        current.children.remove(&child_char);
                    }
                } else if current.document_id.is_none() && current.children.is_empty() {
                    parent.children.remove(&c);
                } else {
                    break;
                }
            }
        }
        Ok(())
    }

    fn trie_collect_paths(&self, node: &ReverseTrieNode, prefix: &str, current_path: String, results: &mut Vec<String>) -> Result<(), StreamDBError> {
        let new_path = format!("{}{}", node.edge, current_path);
        if let Some(_) = node.document_id {
            let path = new_path.chars().rev().collect::<String>();
            if path.starts_with(prefix) {
                results.push(path);
            }
        }
        for (_, &child_index
