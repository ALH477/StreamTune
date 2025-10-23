// Simple CLI for exporting albums or playlists from StreamDB
// Usage: cargo run -- --db /path/to/music.db --type album --key "audio:artist:album" --output /path/to/export
// or: cargo run -- --db /path/to/music.db --type playlist --key "playlist:name" --output /path/to/export

use clap::Parser;
use std::fs::File;
use std::io::{Write, BufWriter};
use std::path::Path;
use streamdb::{StreamDB, Config};
use prost::Message;
use metadata::{Metadata, Playlist}; // Assume from StreamTune's metadata.proto

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to StreamDB file
    #[arg(short, long)]
    db: String,

    /// Type to export: album or playlist
    #[arg(short, long)]
    r#type: String,

    /// Key for album (e.g., "audio:artist:album") or playlist (e.g., "playlist:name")
    #[arg(short, long)]
    key: String,

    /// Output directory
    #[arg(short, long)]
    output: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let config = Config::default();
    let mut db = StreamDB::open_with_config(&args.db, config)?;

    let output_dir = Path::new(&args.output);
    std::fs::create_dir_all(output_dir)?;

    match args.r#type.as_str() {
        "album" => export_album(&mut db, &args.key, output_dir)?,
        "playlist" => export_playlist(&mut db, &args.key, output_dir)?,
        _ => eprintln!("Invalid type: use 'album' or 'playlist'"),
    }

    Ok(())
}

fn export_album(db: &mut StreamDB, prefix: &str, output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let keys = db.prefix_search(prefix.as_bytes())?;
    for key in keys {
        if let Some(data) = db.get(&key)? {
            let mut cursor = Cursor::new(&data);
            let metadata = Metadata::decode_length_delimited(&mut cursor)?;
            let audio = cursor.into_inner()[cursor.position() as usize..].to_vec();

            let filename = format!("{}_{}.opus", metadata.artist.replace(" ", "_"), metadata.title.replace(" ", "_"));
            let file_path = output_dir.join(filename);
            let mut file = BufWriter::new(File::create(file_path)?);
            file.write_all(&audio)?;
            println!("Exported: {}", filename);
        }
    }
    Ok(())
}

fn export_playlist(db: &mut StreamDB, playlist_key: &str, output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(data) = db.get(playlist_key.as_bytes())? {
        let playlist = Playlist::decode_length_delimited(&data[..])?;
        for track_key in playlist.track_keys {
            if let Some(track_data) = db.get(track_key.as_bytes())? {
                let mut cursor = Cursor::new(&track_data);
                let metadata = Metadata::decode_length_delimited(&mut cursor)?;
                let audio = cursor.into_inner()[cursor.position() as usize..].to_vec();

                let filename = format!("{}_{}.opus", metadata.artist.replace(" ", "_"), metadata.title.replace(" ", "_"));
                let file_path = output_dir.join(filename);
                let mut file = BufWriter::new(File::create(file_path)?);
                file.write_all(&audio)?;
                println!("Exported: {}", filename);
            }
        }
    } else {
        eprintln!("Playlist not found");
    }
    Ok(())
}
