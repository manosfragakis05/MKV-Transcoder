fn main() {
    let ffmpeg_dir = "c_src/ffmpeg_src";

    // Tell the rust compiler where to find the static libraries
    println!("cargo:rustc-link-search=native={}/libavcodec", ffmpeg_dir);
    println!("cargo:rustc-link-search=native={}/libavutil", ffmpeg_dir);
    println!(
        "cargo:rustc-link-search=native={}/libswresample",
        ffmpeg_dir
    );

    // Tell the rust compiler which libraries to link into the wasm file
    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=static=swresample");

    // Compile custom C wrapper
    cc::Build::new()
        .file("c_src/decoder.c")
        .include(ffmpeg_dir)
        .compile("custom_decoder");

    // Emscripten Linker Flags
    println!("cargo:rustc-link-arg=-sEXPORT_ES6=1");
    println!("cargo:rustc-link-arg=-sMODULARIZE=1");

    println!("cargo:rustc-link-arg=-sALLOW_MEMORY_GROWTH=1");

    println!("cargo:rustc-link-arg=-sEXPORTED_FUNCTIONS=['_alloc_memory', '_free_memory', '_free_segment', '_get_mkv_info_fast_json', '_free_string', '_parse_cues_json', '_demuxer_create', '_demuxer_destroy', '_demuxer_reset', '_demuxer_init', '_demuxer_parse_chunk', '_demuxer_get_mp4_segment', '_demuxer_get_last_len', '_demuxer_decode_next_audio_frame', '_demuxer_get_last_audio_dts', '_get_audio_ptr', '_demuxer_append_aac', '_demuxer_set_transcode_mode']");

    println!("cargo:rustc-link-arg=-sEXPORTED_RUNTIME_METHODS=['ccall','cwrap','UTF8ToString','allocateUTF8','wasmMemory','HEAPU8']");

    println!("cargo:rustc-link-arg=-sALLOW_MEMORY_GROWTH=1");

    println!("cargo:rustc-link-arg=-sWASM_BIGINT=1");
}
