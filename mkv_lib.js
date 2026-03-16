import Module from './streaming-engine/target/wasm32-unknown-emscripten/release/streaming-engine.js';

let wasmModule = null;

async function init() {
    if (!wasmModule) {
        // In Modularized ES6, we await the factory to get the actual instance
        wasmModule = await Module();
        console.log("🛠️ WASM Instance initialized. Keys:", Object.keys(wasmModule));
    }
    return wasmModule;
}

// BULLETPROOF MEMORY VIEWER
// This guarantees we always have the right memory array, even if it grows or changes names!
function getWasmView() {
    const buffer = wasmModule.HEAPU8 ? wasmModule.HEAPU8.buffer :
        (wasmModule.wasmMemory ? wasmModule.wasmMemory.buffer :
            (wasmModule.memory ? wasmModule.memory.buffer : null));

    if (!buffer) {
        console.error("WASM Module State:", wasmModule);
        throw new Error("Could not find WASM memory buffer! Check console for Module state.");
    }
    return new Uint8Array(buffer);
}

// 3. We recreate your 'Demuxer' class to wrap the raw C-functions
class Demuxer {
    constructor(videoId, audioId, width, height, duration, codecId, needsFfmpeg = false) {
        // 1. Convert the string to raw bytes and add a null-terminator (\0) for C/Rust
        const encoder = new TextEncoder();
        const codecBytes = encoder.encode(codecId + "\0");

        // 2. Use OUR custom Rust memory allocator!
        const codecPtr = wasmModule._alloc_memory(codecBytes.length);
        getWasmView().set(codecBytes, codecPtr);

        // 3. Safe 5-Argument Call
        this.ptr = wasmModule._demuxer_new(videoId, audioId, width, height, codecPtr);

        // 4. Safe Dedicated Toggle Call
        wasmModule._demuxer_set_ffmpeg(this.ptr, needsFfmpeg ? 1 : 0);

        // 5. Use OUR custom Rust free function! No more memory crossing!
        wasmModule._free_memory(codecPtr, codecBytes.length);
    }

    init(headerData) {
        let headerPtr = wasmModule._alloc_memory(headerData.length);
        getWasmView().set(headerData, headerPtr);
        wasmModule._demuxer_init(this.ptr, headerPtr, headerData.length);
        let outPtr = wasmModule._demuxer_get_output_ptr(this.ptr);
        let outLen = wasmModule._demuxer_get_output_len(this.ptr);
        let result = new Uint8Array(getWasmView().buffer, outPtr, outLen).slice();
        wasmModule._free_memory(headerPtr, headerData.length);
        return result;
    }

    parse_chunk(chunkData, isFinal) {
        let chunkPtr = wasmModule._alloc_memory(chunkData.length);
        getWasmView().set(chunkData, chunkPtr);
        let framesStaged = wasmModule._demuxer_parse_chunk(this.ptr, chunkPtr, chunkData.length, isFinal);
        wasmModule._free_memory(chunkPtr, chunkData.length);
        return framesStaged;
    }

    get_mp4_segment() {
        wasmModule._demuxer_get_mp4_segment(this.ptr);
        let outPtr = wasmModule._demuxer_get_output_ptr(this.ptr);
        let outLen = wasmModule._demuxer_get_output_len(this.ptr);
        return new Uint8Array(getWasmView().buffer, outPtr, outLen).slice();
    }

    // NEW: Extract the raw sound waves!
    get_pcm_data() {
        let pcmLen = wasmModule._demuxer_get_pcm_len(this.ptr);
        if (pcmLen === 0) return null;
        let pcmPtr = wasmModule._demuxer_get_pcm_ptr(this.ptr);
        let pcmData = new Float32Array(getWasmView().buffer, pcmPtr, pcmLen).slice();
        wasmModule._demuxer_clear_pcm(this.ptr);
        return pcmData;
    }

    reset() { wasmModule._demuxer_reset(this.ptr); }
}

class MKVFetcher {
    constructor(type, source) {
        this.type = type;
        this.source = source;
        this.size = Infinity; // Default to Infinity to prevent 416 negative ranges!
    }

    async init() {
        if (this.type === 'file') {
            this.size = this.source.size;
            return;
        }

        try {
            // 1. Try the standard HEAD request first
            const res = await fetch(this.source, { method: 'HEAD' });
            const length = parseInt(res.headers.get('content-length'));
            if (length && !isNaN(length)) {
                this.size = length;
                return;
            }
        } catch (e) {
            console.warn("HEAD request failed, trying fallback...");
        }

        try {
            // 2. Fallback: 1-byte GET request
            const controller = new AbortController();
            const res = await fetch(this.source, {
                headers: { 'Range': 'bytes=0-0' },
                signal: controller.signal
            });
            const cr = res.headers.get('content-range');
            if (cr) {
                this.size = parseInt(cr.split('/')[1]);
            }
            controller.abort();
        } catch (e) {
            console.warn("Could not fetch file size. Engine will run in blind mode.");
        }
    }

    async read(start, end, signal) {
        if (this.size !== Infinity && end > this.size) end = this.size;
        if (start >= end) return new Uint8Array(0);

        if (this.type === 'file') {
            return new Uint8Array(await this.source.slice(start, end).arrayBuffer());
        } else {
            const res = await fetch(this.source, {
                headers: { 'Range': `bytes=${start}-${end - 1}` },
                signal: signal
            });

            if (!res.ok) {
                if (res.status === 416) return new Uint8Array(0);
                throw new Error(`HTTP Error ${res.status} for range ${start}-${end - 1}`);
            }
            return new Uint8Array(await res.arrayBuffer());
        }
    }

    async *stream(start, end, signal) {
        if (this.size !== Infinity && end > this.size) end = this.size;
        if (start >= end) return;

        let streamObj;
        if (this.type === 'file') {
            streamObj = this.source.slice(start, end).stream();
        } else {
            const res = await fetch(this.source, {
                headers: { 'Range': `bytes=${start}-${end - 1}` },
                signal: signal
            });

            if (!res.ok) {
                if (res.status === 416) return;
                throw new Error(`HTTP Error ${res.status} for range ${start}-${end - 1}`);
            }
            streamObj = res.body;
        }

        const reader = streamObj.getReader();
        try {
            while (true) {
                const { done, value } = await reader.read();
                if (done) break;
                yield value;
            }
        } finally {
            reader.releaseLock();
        }
    }
}

function readVintJS(buffer, offset, maxOffset) {
    if (offset >= maxOffset) return null;

    const firstByte = buffer[offset];
    let length = 0;

    if (firstByte & 0x80) length = 1;
    else if (firstByte & 0x40) length = 2;
    else if (firstByte & 0x20) length = 3;
    else if (firstByte & 0x10) length = 4;
    else if (firstByte & 0x08) length = 5;
    else if (firstByte & 0x04) length = 6;
    else if (firstByte & 0x02) length = 7;
    else if (firstByte & 0x01) length = 8;
    else return null;

    if (offset + length > maxOffset) return null;

    let value = firstByte & (0xFF >> length);
    for (let i = 1; i < length; i++) {
        value = (value * 256) + buffer[offset + i];
    }

    return { value: value, length: length };
}

class CoreEngine {
    constructor() {
        this.video = null;
        this.chunkSize = 5 * 1024 * 1024;
        this._resetState();
    }

    log(msg) {
        console.log("⚙️ Engine:", msg);
    }

    _resetState() {
        this.isFetching = false;
        if (this.abortController) this.abortController.abort();
        this.abortController = null;
        this.currentStreamId = (this.currentStreamId || 0) + 1;
        this.currentOffset = 0;
        this.cueMap = [];
        if (this.seekTimeout) clearTimeout(this.seekTimeout);
        this.seekTimeout = null;
        this.audioTracks = [];
        this.sourceBuffer = null;
    }

    async preload(fetcher) {
        this._resetState();
        this.sourceInput = fetcher;
        await this.sourceInput.init();

        this.log("Probing for MKV clusters (Dynamic Streamed Preload)...");

        const maxProbe = 50 * 1024 * 1024;
        let capacity = 2 * 1024 * 1024;

        const wasm = await init();
        let ptr = wasm._alloc_memory(capacity);
        let wasmHeap = new Uint8Array(getWasmView().buffer, ptr, capacity);

        let currentSize = 0;
        let absoluteFileOffset = 0;
        let clusterFound = false;

        while (!clusterFound && absoluteFileOffset < this.sourceInput.size && currentSize < maxProbe) {
            const probeController = new AbortController();
            let jumped = false;

            try {
                for await (const chunk of this.sourceInput.stream(absoluteFileOffset, this.sourceInput.size, probeController.signal)) {

                    // 1. WASM Memory reallocation (Bulletproof Edition)
                    if (currentSize + chunk.length > capacity) {
                        let oldPtr = ptr;
                        let oldCapacity = capacity;
                        capacity = Math.max(capacity * 2, currentSize + chunk.length);
                        ptr = wasm._alloc_memory(capacity);

                        let freshBuffer = getWasmView().buffer;

                        let oldView = new Uint8Array(freshBuffer, oldPtr, currentSize);
                        let newWasmHeap = new Uint8Array(freshBuffer, ptr, capacity);
                        newWasmHeap.set(oldView);
                        wasm._free_memory(oldPtr, oldCapacity);
                        wasmHeap = newWasmHeap;
                    } else if (wasmHeap.buffer.byteLength === 0) {
                        wasmHeap = new Uint8Array(getWasmView().buffer, ptr, capacity);
                    }

                    wasmHeap.set(chunk, currentSize);

                    let scanStart = Math.max(0, currentSize - 8);
                    currentSize += chunk.length;
                    absoluteFileOffset += chunk.length;

                    for (let i = scanStart; i < currentSize - 4; i++) {
                        if (wasmHeap[i] === 0x1F && wasmHeap[i + 1] === 0x43 &&
                            wasmHeap[i + 2] === 0xB6 && wasmHeap[i + 3] === 0x75) {
                            clusterFound = true;
                            this.firstClusterOffset = absoluteFileOffset - currentSize + i;
                            probeController.abort();
                            break;
                        }

                        if (wasmHeap[i] === 0x19 && wasmHeap[i + 1] === 0x41 &&
                            wasmHeap[i + 2] === 0xA4 && wasmHeap[i + 3] === 0x69) {

                            const vint = readVintJS(wasmHeap, i + 4, currentSize);

                            if (vint) {
                                if (vint.value < 1024 * 1024) continue;

                                const skipAmount = 4 + vint.length + vint.value;
                                this.log(`🎯 Attachments detected! Size: ${(vint.value / 1024 / 1024).toFixed(2)} MB. Initiating Jump...`);

                                const startOfBufferOffset = absoluteFileOffset - currentSize;
                                absoluteFileOffset = startOfBufferOffset + i + skipAmount;
                                currentSize = i;

                                jumped = true;
                                probeController.abort();
                                break;
                            }
                        }
                    }

                    if (clusterFound || jumped) break;
                    if (currentSize >= maxProbe) break;
                }
            } catch (err) {
                if (err?.name !== 'AbortError') throw err;
            }

            if (clusterFound) break;
        }

        if (!clusterFound) throw new Error("Could not find Video Track.");

        this.log("Parsing metadata via Zero-Copy Rust pointer...");

        this.initialHeaderData = wasmHeap.slice(0, currentSize);

        let jsonPtr = wasm._get_mkv_info_fast(ptr, currentSize);
        let jsonString = wasm.UTF8ToString(jsonPtr);
        this.mkvHeader = JSON.parse(jsonString);

        wasm._free_memory(ptr, capacity);

        const videoTrack = this.mkvHeader.tracks.find(t => t.track_type === "video");
        this.audioTracks = this.mkvHeader.tracks.filter(t => t.track_type === "audio");
        const audioTrack = this.audioTracks.length > 0 ? this.audioTracks[0] : null;

        // FIXED: parse_cues Emscripten Memory Bridge
        if (this.mkvHeader.cues_position) {
            const pos = Number(this.mkvHeader.cues_position);
            const cuesData = await this.sourceInput.read(pos, this.sourceInput.size, null);

            let cuesPtr = wasm._alloc_memory(cuesData.length);
            getWasmView().set(cuesData, cuesPtr);

            let jsonPtrCues = wasm._parse_cues(cuesPtr, cuesData.length);
            let cuesString = wasm.UTF8ToString(jsonPtrCues);

            this.cueMap = JSON.parse(cuesString);
            wasm._free_memory(cuesPtr, cuesData.length);
        }

        const audioId = audioTrack ? BigInt(audioTrack.track_number) : 0n;
        this.demuxer = new Demuxer(
            BigInt(videoTrack.track_number), audioId,
            videoTrack.width, videoTrack.height,
            this.mkvHeader.duration * 1000, videoTrack.codec_id
        );

        this.videoTrack = videoTrack;
        this.audioTrack = audioTrack;

        this.log("✅ Engine Preloaded & Warmed up! Waiting for video tag...");
    }

    async start(needsFfmpeg = false) {
        try {
            if (needsFfmpeg) this.log("🔊 FFmpeg Engine Engaged! Decoding Dolby to PCM...");

            const audioId = this.audioTrack ? BigInt(this.audioTrack.track_number) : 0n;
            this.demuxer = new Demuxer(
                BigInt(this.videoTrack.track_number), audioId,
                this.videoTrack.width, this.videoTrack.height,
                this.mkvHeader.duration * 1000, this.videoTrack.codec_id,
                needsFfmpeg
            );

            const initSegment = this.demuxer.init(this.initialHeaderData);
            await this._appendToBuffer(initSegment);

            this.currentOffset = this.cueMap.length > 0
                ? Math.max(0, Number(this.cueMap[0].offset) - 100000)
                : (this.firstClusterOffset || 0);

            this.log("▶️ Worker Engine Started! Pumping chunks to Main Thread...");
            this._streamLoop();

        } catch (error) {
            console.error("Worker Engine Crash:", error);
        }
    }

    async _streamLoop() {
        let myStreamId = this.currentStreamId;
        if (this.isFetching || this.currentOffset >= this.sourceInput.size) return;
        this.isFetching = true;

        while (this.currentOffset < this.sourceInput.size && myStreamId === this.currentStreamId) {

            let limit = (this.bufferedEnd || 0) - (this.currentTime || 0);

            if (limit > 60 || this.queueLength > 5) {
                break;
            }

            this.abortController = new AbortController();
            try {
                let bytesProcessed = 0;

                for await (const chunkData of this.sourceInput.stream(this.currentOffset, this.currentOffset + this.chunkSize, this.abortController.signal)) {
                    if (myStreamId !== this.currentStreamId) break;

                    const isFinal = (this.currentOffset + bytesProcessed + chunkData.length) >= this.sourceInput.size;
                    const framesStaged = this.demuxer.parse_chunk(chunkData, isFinal);

                    const pcmData = this.demuxer.get_pcm_data();
                    if (pcmData && this.onAudioReady) {
                        this.onAudioReady(pcmData);
                    }

                    if (framesStaged >= 30 || isFinal) {
                        const segment = this.demuxer.get_mp4_segment();

                        if (segment.length > 0) {
                            await this._appendToBuffer(segment);
                            await new Promise(r => setTimeout(r, 0));
                        }
                    }
                    bytesProcessed += chunkData.length;
                }
                this.currentOffset += bytesProcessed;
            } catch (err) {
                if (err?.name === 'AbortError') break;
                else { console.error(err); break; }
            } finally {
                this.abortController = null;
            }
        }
        if (myStreamId === this.currentStreamId) this.isFetching = false;
    }

    syncProgress(currentTime, bufferedEnd, queueLength) {
        this.currentTime = currentTime;
        this.bufferedEnd = bufferedEnd;
        this.queueLength = queueLength || 0;
        this._streamLoop();
    }

    seekTo(targetTime) {
        this.log(`🕒 Worker Seeking to ${targetTime}s...`);
        if (this.abortController) { this.abortController.abort(); this.abortController = null; }
        if (this.seekTimeout) clearTimeout(this.seekTimeout);

        this.seekTimeout = setTimeout(async () => {
            this.currentStreamId++;
            if (this.demuxer) this.demuxer.reset();

            let bestCue = this.cueMap[0];
            for (let i = 0; i < this.cueMap.length; i++) {
                if (this.cueMap[i].time <= targetTime) bestCue = this.cueMap[i];
                else break;
            }

            // 🚨 FIX 1: Jump exactly to the Keyframe, no minus 100k hack!
            this.currentOffset = bestCue ? Number(bestCue.offset) : 0;

            // 🚨 FIX 2: Tell the main thread to snap the video to this exact Keyframe time
            if (bestCue) {
                postMessage({ type: 'SEEK_SNAP', payload: { time: bestCue.time } });
            }

            await new Promise(r => setTimeout(r, 300));
            this.isFetching = false;
            this._streamLoop();
        }, 500);
    }

    async switchAudioTrack(newTrackNumber, currentVideoTime) {
        this.log(`🎧 Worker: Hot-Swap Audio to ID: ${newTrackNumber}`);

        if (this.abortController) {
            this.abortController.abort();
            this.abortController = null;
        }
        this.currentStreamId++;

        const newAudioTrack = this.audioTracks.find(t => t.track_number === newTrackNumber);
        if (!newAudioTrack) return console.error("Track not found!");
        this.audioTrack = newAudioTrack;

        this.demuxer = new Demuxer(
            BigInt(this.videoTrack.track_number), BigInt(newAudioTrack.track_number),
            this.videoTrack.width, this.videoTrack.height,
            this.mkvHeader.duration * 1000, this.videoTrack.codec_id
        );

        const newInitSegment = this.demuxer.init(this.initialHeaderData);

        if (this.onAudioSwap) {
            this.onAudioSwap(newInitSegment, newAudioTrack.codec_string);
        }

        this.seekTo(currentVideoTime);
    }

    async _appendToBuffer(data) {
        if (this.onSegmentReady) {
            this.onSegmentReady(data);
        }
    }
}

const streamDictionary = new Map();
let isWasmLoaded = false;

export async function feed(url) {
    if (!isWasmLoaded) { await init(); isWasmLoaded = true; }
    if (streamDictionary.has(url)) return streamDictionary.get(url);

    console.log(`[SDK] Feeding stream: ${url}`);
    const fetcher = new MKVFetcher('url', url);
    const engine = new CoreEngine();
    await engine.preload(fetcher);

    streamDictionary.set(url, engine);
    return engine;
}