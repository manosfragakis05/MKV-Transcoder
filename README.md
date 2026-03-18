# 🎬 In-Browser MKV Player (Serverless WASM Transcoder)

A blazing-fast, serverless, Just-In-Time (JIT) video transcoder that runs entirely in the browser. 

This engine takes standard Matroska (`.mkv`) video streams, demuxes them using a custom Rust/C WebAssembly pipeline, transcodes unsupported audio on the fly using the browser's hardware-accelerated WebCodecs API, and feeds standard Fragmented MP4 (`fMP4`) chunks directly into an HTML5 `<video>` tag via MediaSource Extensions (MSE).

Absolutely no backend server required. It works completely offline.

## ✨ Key Features

* **Zero-Copy Memory Architecture:** Passes memory pointers directly from C/Rust into JavaScript `Float32Arrays`, completely eliminating expensive RAM duplication during the transcoding loop.
* **Serverless & Offline-Ready:** Because the engine runs entirely on the client's CPU/GPU, it can be deployed as a static site or PWA. Drop a local 2GB MKV file into it on an airplane, and it will play natively.
* **Dynamic Audio Polymorphism:** * Automatically detects hardware capabilities. 
  * Triggers the WebAssembly transcoder (to AAC) only when necessary.
  * **Apple Safari Cheat Code:** Natively passes through Dolby Digital (AC3) and Dolby Digital Plus (EAC3) on iOS and macOS, bypassing the transcoder entirely for 0% CPU overhead.
* **Hardware-Accelerated Encoding:** Bypasses heavy software encoders by piping raw PCM audio into the browser's native `AudioEncoder` for rapid AAC compression.

## 🏗️ Architecture Pipeline

1. **Network/Disk:** Fetches binary chunks of an MKV file via Fetch API or local File interface.
2. **WASM Demuxer:** Rust parses the EBML Matroska clusters and extracts the compressed video/audio frames.
3. **Hardware Routing:**
   * *If Direct Play Supported:* Rust packages the raw frames into MP4 `moof`/`mdat` boxes.
   * *If Transcode Required:* FFmpeg (C) decodes the audio to raw PCM -> JS WebCodecs encodes to AAC -> Rust packages into MP4 boxes.
4. **Playback:** JS pushes the final MP4 byte-stream into the `SourceBuffer` for seamless playback.

## 🚀 Future Roadmap (V2.0)

* [ ] **Standard MP4 Multiplexing:** Rewrite the remuxer to build a master `moov` atom at the end of the file, allowing users to download the full, transcoded `.mp4` for offline hardware players (iPods, TVs).
* [ ] **Dynamic Audio Resampling:** Compile `libswresample` into the WASM pipeline to support downsampling rare 96kHz/88.2kHz tracks to the standard 48kHz for the WebCodecs API.
* [ ] **Manual Garbage Collection:** Implement a custom `SourceBuffer` eviction loop for aggressive memory management on extreme low-end mobile devices (<3GB RAM).

## 🛠️ Build Instructions

To build this project from source, you will need Rust, `wasm-pack`, and Emscripten installed.

**Note on FFmpeg:** This repository does *not* include the FFmpeg C-source code. To compile the decoder, you must download the FFmpeg source, compile it to WebAssembly using Emscripten, and place the resulting static libraries (`.a`) and headers in the `c_src/ffmpeg_src/` directory before running `cargo build`.

## ⚖️ License & Acknowledgements

This project's custom Rust and JavaScript codebase is open-source. 

**FFmpeg Disclaimer:**
This software uses libraries from the [FFmpeg](http://ffmpeg.org) project under the **LGPLv2.1** license. FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project. The pre-compiled WebAssembly binaries in this project dynamically link to FFmpeg's decoders.