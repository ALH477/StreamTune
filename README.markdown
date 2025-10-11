# StreamTune: Lightweight Music Player/Manager

StreamTune is a cross-platform application for organizing and playing music, built with Flutter for a consistent user interface across mobile (iOS, Android) and desktop (Windows, macOS, Linux), and Rust for performance-critical operations. It leverages StreamDB (LGPLv3), a lightweight key-value database owned by DeMoD LLC, to store Opus-encoded audio and Protobuf-serialized metadata, ensuring compact storage and fast queries. The app includes beta DSP effects (delay, reverb, chorus) and a 3-band EQ inspired by DeMoD EchoForge, delivering high-quality (HQ) audio processing. StreamTune is licensed under LGPLv3, allowing proprietary linking while requiring source disclosure for modifications.

StreamTune is designed for musicians and audiophiles seeking a robust, offline-first music management solution with low-latency playback and extensible DSP capabilities. It supports small-to-medium libraries (<10k tracks) with a focus on efficiency, reliability, and community-driven development.

## Repo Structure
``` 
streamtune/
├── LICENSE                    # LGPLv3 license text
├── README.md                  # Detailed project documentation
├── CONTRIBUTING.md            # Contribution guidelines
├── rust/                      # Rust backend crate
│   ├── Cargo.toml             # Rust dependencies and metadata
│   ├── build.rs               # Protobuf codegen
│   ├── src/
│   │   ├── api.rs             # Core logic (AppState, track/playlist/DSP functions)
│   │   ├── metadata.proto     # Protobuf schema for metadata, playlists, EQ
│   │   └── streamdb.rs        # StreamDB with trie indexing, paged storage
├── flutter/                   # Flutter app (mobile and desktop support)
│   ├── pubspec.yaml           # Flutter dependencies
│   ├── lib/
│   │   ├── main.dart          # Main app UI
│   │   └── dsp_beta.dart      # Beta DSP/EQ settings screen
│   ├── android/               # Android-specific configs
│   │   └── app/build.gradle   # Android build settings
│   ├── ios/                   # iOS-specific configs
│   │   └── Runner/Info.plist  # iOS permissions
│   ├── macos/                 # macOS-specific configs
│   │   └── Runner/Info.plist  # macOS permissions
│   ├── windows/               # Windows-specific configs
│   │   └── runner/main.cpp    # Windows entry point
│   └── linux/                 # Linux-specific configs
│       └── my_application.cc  # Linux entry point
└── bridge_generated/          # FRB-generated bindings (auto-generated, not included)
```


## Table of Contents
- [Features](#features)
- [Architecture](#architecture)
- [Prerequisites](#prerequisites)
- [Setup](#setup)
- [Usage](#usage)
- [Dependencies](#dependencies)
- [Build and Deployment](#build-and-deployment)
- [Desktop Support](#desktop-support)
- [Testing](#testing)
- [Scalability](#scalability)
- [Mobile and Desktop Optimizations](#mobile-and-desktop-optimizations)
- [License](#license)
- [Credits](#credits)
- [Contributing](#contributing)
- [Known Limitations](#known-limitations)
- [Contact](#contact)

## Features
- **Audio Ingestion**: Import audio files (MP3, WAV, AAC, OGG) from device/storage, automatically encoded to Opus (128kbps VBR) for compact, high-quality storage in StreamDB.
- **Playlists**: Create, edit, and manage playlists with prefix-searchable keys (e.g., `playlist:favorites`).
- **Search/Browsing**: Fast prefix searches for artists, albums, and tracks (e.g., `audio:artist:B*` for Bob Dylan).
- **Playback**: Native playback via `audioplayers` with low latency (<50ms track start); supports queuing and prefetching for seamless transitions.
- **Beta DSP Effects**: Togglable effects (delay: 0.5s, reverb: 0.8 mix, chorus: 5Hz rate) and 3-band EQ (bass: 100Hz, mid: 1kHz, treble: 5kHz; -12dB to +12dB gains).
- **Robustness**: Comprehensive error handling (file errors, decode failures, DSP issues); logging to app directory; recovery (e.g., skips invalid tracks).
- **Platforms**: iOS (18+), Android (10+), Windows (10+), macOS (12+), Linux (Ubuntu 22.04+).
- **Storage**: Offline-first with StreamDB; extensible for cloud sync (e.g., Firebase).
- **EchoForge Integration**: Designed for future USB-C audio routing to DeMoD’s EchoForge hardware (I2S at 48kHz).

## Architecture
- **UI Layer (Flutter/Dart)**: Reactive interface using Riverpod for state management; handles track listing, playlist management, playback controls, and DSP settings. Flutter ensures a consistent UX across mobile and desktop, similar to Qt’s cross-platform capabilities.
- **Bridge (flutter_rust_bridge)**: Auto-generates Dart bindings for Rust APIs, enabling async calls and Protobuf struct serialization.
- **Core (Rust)**:
  - **StreamDB**: Stores Opus audio blobs, metadata, playlists, and DSP configs with trie-based indexing for fast prefix searches.
  - **Audio Processing**: Symphonia for decoding, libopusenc for Opus encoding, fundsp for DSP (delay, reverb, chorus, EQ).
  - **Metadata**: Protobuf for compact serialization (title, artist, album, duration).
- **Storage**: StreamDB file in app’s documents directory (e.g., `%APPDATA%/StreamTune/music.db` on Windows, `/data/user/0/com.demod.streamtune/files/music.db` on Android).
- **Playback**: Rust streams Opus data to Flutter’s `audioplayers` for native playback.

## Prerequisites
- **Rust**: 1.80+ (install via `rustup.rs`).
- **Flutter**: 3.24+ (install via `flutter.dev`).
- **Desktop Platforms**:
  - Windows: Visual Studio (Community) with C++ Desktop Development workload.
  - macOS: Xcode 15.0+.
  - Linux: GTK3 development libraries (`sudo apt install libgtk-3-dev` on Ubuntu).
- **Mobile Platforms**: Android SDK (API 33+), Xcode (15.0+).
- **Tools**: `flutter_rust_bridge_codegen` (install via `cargo install flutter_rust_bridge_codegen`).
- **Git**: For cloning the repository.

## Setup
1. **Clone the Repository**:
   ```bash
   git clone https://github.com/DeMoD/StreamTune.git
   cd streamtune
   ```

2. **Enable Desktop Support in Flutter**:
   ```bash
   flutter config --enable-windows-desktop
   flutter config --enable-macos-desktop
   flutter config --enable-linux-desktop
   flutter create .
   ```
   - Adds platform-specific folders (`windows/`, `macos/`, `linux/`).

3. **Set Up Rust Backend**:
   ```bash
   cd rust
   cargo build --release
   ```
   - Builds StreamDB and core logic as a `cdylib` for mobile/desktop linking.

4. **Set Up Flutter Frontend**:
   ```bash
   cd ../flutter
   flutter pub get
   ```

5. **Generate Flutter-Rust-Bridge Bindings**:
   ```bash
   flutter_rust_bridge_codegen \
     --rust-input rust/src/api.rs \
     --dart-output flutter/lib/bridge_generated.dart \
     --c-output flutter/ios/Runner/bridge_generated.h \
     --llvm-path /usr/local/opt/llvm  # Adjust for your LLVM path (e.g., on macOS)
   ```

6. **Verify Dependencies**:
   - Rust: `prost`, `symphonia`, `libopusenc`, `fundsp`, `streamdb`, etc.
   - Flutter: `file_picker`, `audioplayers`, `riverpod`, `flutter_rust_bridge`, `permission_handler`.

## Usage
1. **Launch App**:
   - **Desktop**: `flutter run -d windows` (or `macos`, `linux`).
   - **Mobile**: `flutter run --release` (connect device/emulator).
   - Requests storage/audio permissions on mobile; desktop uses local file access.

2. **Add Tracks**:
   - Click “Add Track” to select audio files (MP3, WAV, AAC, OGG).
   - Files are decoded, encoded to Opus, and stored in StreamDB with metadata.

3. **Create/Edit Playlists**:
   - Click “Create Playlist” to start a new playlist.
   - Use “+” icon on tracks to add to playlists (e.g., `playlist:My Playlist`).

4. **Browse/Search**:
   - View tracks in a list; click to play.
   - Search by prefix (extendable with search bar in future updates).

5. **Playback**:
   - Mini-player provides play/pause, next/prev controls.
   - Prefetches next track for gapless playback.

6. **Beta DSP/EQ**:
   - Navigate to Settings > DSP Beta.
   - Toggle DSP (delay, reverb, chorus) or EQ (bass/mid/treble sliders).
   - Changes apply instantly to playback; saved in StreamDB.

7. **Logs**:
   - Debug logs in `<app_documents_dir>/music.db.log` (e.g., `%APPDATA%/StreamTune/music.db.log` on Windows).

## Dependencies
- **Rust** (`rust/Cargo.toml`):
  - `prost` (0.13.3): Protobuf serialization.
  - `symphonia` (0.5.4): Audio decoding (MP3, WAV, AAC, OGG).
  - `libopusenc` (0.2.1): Opus encoding.
  - `fundsp` (0.11.0): DSP effects (delay, reverb, chorus, EQ).
  - `cpal` (0.15.3): Audio I/O for potential live input.
  - `tracing` (0.1.40), `tracing-subscriber` (0.3.18): Logging.
  - `rodio` (0.18.1, with `opus`): Opus decoding for DSP.
  - `uuid` (1.10.0), `crc` (3.2.1), `memmap2` (0.9.4), `byteorder` (1.5.0), `parking_lot` (0.12.3), `lru` (0.12.4), `snappy` (0.5.0), `futures` (0.3.30): StreamDB dependencies.
  - `streamdb`: Custom key-value store (LGPLv3, in `rust/streamdb/`).
  - `thiserror`, `anyhow`, `bincode`: Error handling and serialization.
- **Flutter** (`flutter/pubspec.yaml`):
  - `file_picker` (8.1.2): File selection UI.
  - `audioplayers` (6.1.0): Native audio playback.
  - `riverpod` (2.5.1): State management.
  - `flutter_rust_bridge` (2.4.0): Rust-Dart integration.
  - `path_provider` (2.1.4): App directory access.
  - `permission_handler` (10.4.0): Storage/audio permissions (mobile only).

## Build and Deployment
1. **Desktop Builds**:
   - **Windows**:
     - Build: `flutter build windows --release` (outputs in `flutter/build/windows`).
     - Package: Use `msix` (`flutter pub run msix:create`) for Microsoft Store or standalone EXE.
   - **macOS**:
     - Build: `flutter build macos --release` (outputs in `flutter/build/macos`).
     - Package: Create DMG (`hdiutil create`) or sign for App Store.
   - **Linux**:
     - Build: `flutter build linux --release` (outputs in `flutter/build/linux`).
     - Package: Create AppImage or deb/rpm for distribution.
   - Sign: Configure signing keys for each platform (e.g., Windows EV certificate, macOS Developer ID).

2. **Mobile Builds**:
   - **Android**: `flutter build apk --release`.
     - Sign: Configure key in `flutter/android/app/build.gradle`:
       ```gradle
       signingConfigs {
           release {
               keyAlias 'myalias'
               keyPassword 'mypassword'
               storeFile file('/path/to/keystore.jks')
               storePassword 'mypassword'
           }
       }
       buildTypes {
           release {
               signingConfig signingConfigs.release
           }
       }
       ```
     - Deploy: Upload to Google Play.
   - **iOS**: `flutter build ipa --release`.
     - Sign: Configure in Xcode (`flutter/ios/Runner.xcodeproj`).
     - Deploy: Submit to App Store Connect.

3. **LGPL Compliance**:
   - Include StreamDB source in `rust/streamdb/` or provide a link (e.g., `https://github.com/DeMoD/StreamDB`).
   - Add in-app notice: “StreamTune uses StreamDB (LGPLv3). Source available at [URL].”
   - Ensure dynamic linking (`cdylib`) for StreamDB and StreamTune in builds (handled by flutter_rust_bridge).

## Desktop Support
StreamTune uses Flutter Desktop for a cross-compatible UX, providing a native look and feel across Windows, macOS, Linux, iOS, and Android, akin to Qt’s multi-platform capabilities. The Rust backend ensures consistent performance across all platforms.

- **Enabling Desktop**: `flutter create .` adds platform folders (`windows/`, `macos/`, `linux/`).
- **Customizations**: UI auto-scales for desktop resolutions; consider adding keyboard shortcuts (e.g., space for play/pause) in `main.dart` for enhanced desktop UX.
- **Performance**: Desktop hardware (higher RAM/CPU) reduces DSP latency (<2ms vs. 5ms on mobile).
- **Alternative Rust GUI**: For a pure Rust/Qt interface, use `cxx-qt` for Qt bindings, rewriting the UI to call the existing Rust backend. Contact for implementation.

## Testing
- **Unit Tests**:
  - Rust: `cargo test` (tests for `add_track`, `get_playlist`, `apply_dsp_to_track`, `streamdb` operations).
  - Flutter: `flutter test` (UI widget tests for track list, DSP settings).
- **Integration Tests**:
  - Ingest 1k tracks; verify playback latency (<50ms), DSP overhead (<5ms mobile, <2ms desktop).
  - Test playlist creation, track addition, prefix searches, and EQ toggling.
- **Edge Cases**:
  - Invalid audio files (e.g., corrupted MP3, non-audio files).
  - Low memory (2GB RAM mobile, 4GB desktop).
  - Extreme EQ gains (handled by validation at -12dB/+12dB).
  - Interrupted I/O (e.g., storage permission denied, file corruption).
- **Cross-Platform Testing**:
  - Mobile: Android 10+ (min 2GB RAM), iOS 18+.
  - Desktop: Windows 10+, macOS 12+, Ubuntu 22.04+.
  - Verify permissions (mobile only), playback quality, and UI consistency.

## Scalability
- **Local**:
  - StreamDB’s trie-based indexing supports 10k+ tracks with <10ms query latency.
  - Pagination in `search_tracks` (limit 50 results) ensures UI responsiveness.
  - In-memory StreamDB mode for small libraries (<1GB).
- **Cloud**:
  - Extend with Firebase for multi-device playlist sync (proprietary service, outside LGPL scope).
  - Store StreamDB snapshots as blobs; sync deltas for efficiency.
- **Performance**:
  - Streamed DSP processing minimizes memory (O(1) per frame).
  - Quick mode for metadata reads (<10ms).
  - Tested for 10k tracks to ensure linear scaling.

## Mobile and Desktop Optimizations
- **Permissions**: Mobile uses `permission_handler` for storage/audio; desktop relies on local file access.
- **Battery/Performance**: Streamed DSP and playback pause on app suspend/minimize reduce CPU usage (<10% per hour on mid-range mobile, <5% on desktop).
- **Low-End Devices**: Optimized for 2GB RAM (mobile) and 4GB (desktop); in-memory StreamDB for small libraries.
- **Background Playback**: Configured via `audioplayers` (AndroidManifest.xml, Info.plist for mobile; supported natively on desktop).
- **Latency**: <50ms track start, <5ms DSP overhead (mobile), <2ms (desktop).

## License
StreamTune is licensed under the **GNU Lesser General Public License v3.0 (LGPLv3)**. See [LICENSE](LICENSE) for details. This allows proprietary applications to link dynamically to StreamTune or StreamDB, while requiring modifications to either to be released under LGPLv3.

**StreamDB**, a core dependency owned by DeMoD LLC, is also licensed under LGPLv3. Its source is included in `rust/streamdb/` or available at [https://github.com/DeMoD/StreamDB]. As required by LGPLv3, DeMoD LLC provides StreamDB’s source with this distribution.

## Credits
- **DeMoD LLC**: Core development, StreamDB, EchoForge-inspired DSP.
- **xAI**: Inspiration for efficient, scalable systems.
- **StreamDB**: Lightweight key-value store by DeMoD LLC (LGPLv3).
- **Contributors**: [Add GitHub contributors or list, e.g., Asher LeRoy].

## Contributing
We welcome contributions to enhance StreamTune’s features, performance, or usability. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines. Suggested contributions:
- New DSP effects (e.g., distortion, flanger via fundsp).
- UI improvements (e.g., visualizations, playlist drag-drop, keyboard shortcuts for desktop).
- EchoForge hardware integration (USB-C audio routing, I2S at 48kHz).
- Performance optimizations (e.g., faster StreamDB queries).

## Known Limitations
- **Cloud Sync**: Not implemented; extend with Firebase for multi-device support.
- **DSP Beta**: Software-only; EchoForge hardware integration planned (requires USB-C audio routing).
- **Static Linking**: Requires re-linkable binaries for LGPL compliance (use dynamic linking for simplicity).
- **Large Libraries**: Optimized for <10k tracks; larger datasets need cloud or sharding.

## Contact
- **Issues**: File at [https://github.com/DeMoD/StreamTune/issues].
- **Commercial Inquiries**: Contact DeMoD LLC at [email/contact form].
- **Community**: Join discussions on [GitHub Discussions or relevant forum].
