fn main() {
    // We REMOVED the early return because we DO want to compile
    // the C wrapper and link FFmpeg when building for WASM!

    let ffmpeg_dir = "c_src/ffmpeg_src";

    // 1. Tell the Rust compiler where to find the static libraries (.a files)
    println!("cargo:rustc-link-search=native={}/libavcodec", ffmpeg_dir);
    println!("cargo:rustc-link-search=native={}/libavutil", ffmpeg_dir);
    println!(
        "cargo:rustc-link-search=native={}/libswresample",
        ffmpeg_dir
    );

    // 2. Tell the Rust compiler exactly which libraries to link into our Wasm file
    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=static=swresample");

    // 3. Compile our custom C wrapper (decoder.c)
    // When targeting wasm32-unknown-emscripten, this automatically uses 'emcc'!
    cc::Build::new()
        .file("c_src/decoder.c")
        .include(ffmpeg_dir) // This lets decoder.c find the <libavcodec/avcodec.h> headers
        .compile("custom_decoder");

    // Force Emscripten to build a modern ES6 module!
    println!("cargo:rustc-link-arg=-sEXPORT_ES6=1");
    println!("cargo:rustc-link-arg=-sMODULARIZE=1");

    // TELL EMSCRIPTEN TO STOP HIDING THINGS AND EXPORT EVERYTHING (INCLUDING HEAPU8!)
    println!("cargo:rustc-link-arg=-sEXPORT_ALL=1");

    // Unlock the RAM limit
    println!("cargo:rustc-link-arg=-sALLOW_MEMORY_GROWTH=1");
} // <-- End of fn main()
