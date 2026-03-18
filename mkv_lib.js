import initModule from './streaming-engine/target/wasm32-unknown-emscripten/release/streaming-engine.js';
let wasm = null;

// Memory unlocker
function getWasmMemory() {
    if (wasm.HEAPU8 && wasm.HEAPU8.buffer) return wasm.HEAPU8.buffer;
    if (wasm.asm && wasm.asm.memory && wasm.asm.memory.buffer) return wasm.asm.memory.buffer;
    if (wasm.memory && wasm.memory.buffer) return wasm.memory.buffer;
    if (wasm.wasmMemory && wasm.wasmMemory.buffer) return wasm.wasmMemory.buffer;
    console.error("🔍 WASM Object Dump:", wasm);
    throw new Error("CRITICAL: Emscripten memory buffer not found! Check the console dump.");
}

class MKVFetcher {
    constructor(source) {
        this.source = source;
        this.type = source instanceof File ? 'file' : 'url';
        this.size = Infinity;
    }

    async init() {
        if (this.source instanceof File) {
            this.type = 'file';
            this.size = this.source.size;
            return;
        }

        try {
            const res = await fetch(this.source, {
                method: 'HEAD',
                signal: AbortSignal.timeout(3000)
            });
            const length = parseInt(res.headers.get('content-length'));
            if (length && !isNaN(length)) {
                this.size = length;
                return;
            }
        } catch (e) {
            console.warn("HEAD request ignored or timed out. Dropping to GET fallback...");
        }

        try {
            const controller = new AbortController();
            const res = await fetch(this.source, {
                headers: { 'Range': 'bytes=0-0' },
                signal: controller.signal
            });
            const cr = res.headers.get('content-range');
            if (cr) this.size = parseInt(cr.split('/')[1]);
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

class Demuxer {
    constructor(videoId, audioId, width, height, duration, codecId) {
        const encoder = new TextEncoder();
        const codecBytes = encoder.encode(codecId + "\0");
        const codecPtr = wasm._alloc_memory(codecBytes.length);
        new Uint8Array(getWasmMemory(), codecPtr, codecBytes.length).set(codecBytes);

        this.ptr = wasm._demuxer_create(
            BigInt(videoId), BigInt(audioId),
            Number(width), Number(height),
            Number(duration), codecPtr
        );

        wasm._free_memory(codecPtr, codecBytes.length);
    }

    setTranscodeMode(needsTranscode) {
        wasm._demuxer_set_transcode_mode(this.ptr, needsTranscode);
    }

    _handleBufferResult(ptr) {
        if (ptr === 0) return new Uint8Array(0);
        const len = wasm._demuxer_get_last_len(this.ptr);
        if (len === 0) return new Uint8Array(0);

        const data = new Uint8Array(getWasmMemory(), ptr, len).slice();
        wasm._free_segment(ptr, len);
        return data;
    }

    init(chunkData) {
        const chunkPtr = wasm._alloc_memory(chunkData.length);
        new Uint8Array(getWasmMemory(), chunkPtr, chunkData.length).set(chunkData);
        const ptr = wasm._demuxer_init(this.ptr, chunkPtr, chunkData.length);
        wasm._free_memory(chunkPtr, chunkData.length);
        return this._handleBufferResult(ptr);
    }

    get_mp4_segment() {
        const ptr = wasm._demuxer_get_mp4_segment(this.ptr);
        return this._handleBufferResult(ptr);
    }

    parse_chunk(chunkData, isFinal) {
        const chunkPtr = wasm._alloc_memory(chunkData.length);
        new Uint8Array(getWasmMemory(), chunkPtr, chunkData.length).set(chunkData);
        const frames = wasm._demuxer_parse_chunk(this.ptr, chunkPtr, chunkData.length, isFinal);
        wasm._free_memory(chunkPtr, chunkData.length);
        return frames;
    }

    reset() { wasm._demuxer_reset(this.ptr); }
    destroy() { wasm._demuxer_destroy(this.ptr); }
}

class CoreEngine {
    constructor() {
        this.video = null;
        this.chunkSize = 5 * 1024 * 1024;

        this.downloadBuffer = [];
        this.isRecording = false;
        this.mp4InitSegment = null;

        this._resetState();
    }

    log(msg) { console.log("Engine:", msg); }

    _resetState() {
        this.isFetching = false;
        if (this.abortController) this.abortController.abort();
        this.abortController = null;
        this.currentStreamId = (this.currentStreamId || 0) + 1;
        this.currentOffset = 0;
        this.cueMap = [];
        this.audioTracks = [];
        this.sourceBuffer = null;
    }

    _bootAudioEncoder() {
        if (this.audioEncoder && this.audioEncoder.state !== 'closed') {
            try { this.audioEncoder.close(); } catch (e) { }
        }

        this.audioEncoder = new AudioEncoder({
            output: (chunk, metadata) => {
                const aacData = new Uint8Array(chunk.byteLength);
                chunk.copyTo(aacData);
                const aacPtr = wasm._alloc_memory(aacData.length);
                new Uint8Array(getWasmMemory(), aacPtr, aacData.length).set(aacData);

                const dtsSamples = BigInt(Math.floor((chunk.timestamp * 48000) / 1000000));

                wasm._demuxer_append_aac(this.demuxer.ptr, aacPtr, aacData.length, dtsSamples);
                wasm._free_memory(aacPtr, aacData.length);
            },
            error: (e) => console.error("Hardware Encoder Error:", e)
        });

        this.audioEncoder.configure({
            codec: 'mp4a.40.2',
            sampleRate: 48000,
            numberOfChannels: 2,
            bitrate: 128000
        });
        this.log("Hardware Audio Encoder rebuilt.");
    }

    attachVideo(videoElement) {
        this.video = videoElement;
        this.video.disableRemotePlayback = true;
        this.video.onseeking = () => this._onSeeking();
        this.video.ontimeupdate = () => this._onTimeUpdate();
        this.video.src = URL.createObjectURL(this.mediaSource);
        this.log("Video tag attached. Stream routed to screen.");
    }

    async preload(fetcher) {
        this._resetState();
        this.sourceInput = fetcher;
        await this.sourceInput.init();

        this.log("Probing for MKV clusters...");

        const maxProbe = 100 * 1024 * 1024;
        let capacity = 2 * 1024 * 1024;

        if (!wasm) wasm = await initModule();
        let ptr = wasm._alloc_memory(capacity);
        let memBuffer = getWasmMemory();
        let wasmHeap = new Uint8Array(memBuffer, ptr, capacity);

        let currentSize = 0;
        let absoluteFileOffset = 0;
        let clusterFound = false;
        let firstClusterIndex = 0; // The clean slice marker

        while (!clusterFound && absoluteFileOffset < this.sourceInput.size && currentSize < maxProbe) {
            const probeController = new AbortController();
            let jumped = false;

            try {
                for await (const chunk of this.sourceInput.stream(absoluteFileOffset, this.sourceInput.size, probeController.signal)) {

                    if (currentSize + chunk.length > capacity) {
                        let oldPtr = ptr;
                        let oldCapacity = capacity;
                        capacity = Math.max(capacity * 2, currentSize + chunk.length);
                        ptr = wasm._alloc_memory(capacity);
                        let freshBuffer = getWasmMemory();
                        let oldView = new Uint8Array(freshBuffer, oldPtr, currentSize);
                        let newWasmHeap = new Uint8Array(freshBuffer, ptr, capacity);
                        newWasmHeap.set(oldView);
                        wasm._free_memory(oldPtr, oldCapacity);
                        wasmHeap = newWasmHeap;
                    } else if (wasmHeap.buffer.byteLength === 0) {
                        wasmHeap = new Uint8Array(wasm.memory.buffer, ptr, capacity);
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
                            firstClusterIndex = i; // Save exact byte where headers end

                            probeController.abort();
                            break;
                        }

                        if (wasmHeap[i] === 0x19 && wasmHeap[i + 1] === 0x41 &&
                            wasmHeap[i + 2] === 0xA4 && wasmHeap[i + 3] === 0x69) {

                            const vint = readVintJS(wasmHeap, i + 4, currentSize);
                            if (vint) {
                                if (vint.value < 1024 * 1024) continue;

                                const skipAmount = 4 + vint.length + vint.value;
                                this.log(`🎯 Attachments skipped! Size: ${(vint.value / 1024 / 1024).toFixed(2)} MB.`);

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

        // Clean slice so no cluster ends into the headers
        this.initialHeaderData = wasmHeap.slice(0, firstClusterIndex);

        let jsonPtr = wasm._get_mkv_info_fast_json(ptr, currentSize);
        this.mkvHeader = JSON.parse(wasm.UTF8ToString(jsonPtr));
        wasm._free_string(jsonPtr);
        wasm._free_memory(ptr, capacity);

        const videoTrack = this.mkvHeader.tracks.find(t => t.track_type === "video");
        this.audioTracks = this.mkvHeader.tracks.filter(t => t.track_type === "audio");
        const audioTrack = this.audioTracks.length > 0 ? this.audioTracks[0] : null;

        if (this.mkvHeader.cues_position) {
            const pos = Number(this.mkvHeader.cues_position);
            const cuesData = await this.sourceInput.read(pos, this.sourceInput.size, null);

            const cPtr = wasm._alloc_memory(cuesData.length);
            new Uint8Array(getWasmMemory(), cPtr, cuesData.length).set(cuesData);

            let cJsonPtr = wasm._parse_cues_json(cPtr, cuesData.length);
            this.cueMap = JSON.parse(wasm.UTF8ToString(cJsonPtr));

            wasm._free_string(cJsonPtr);
            wasm._free_memory(cPtr, cuesData.length);
        }

        const audioId = audioTrack ? BigInt(audioTrack.track_number) : 0n;
        this.demuxer = new Demuxer(
            BigInt(videoTrack.track_number), audioId,
            videoTrack.width, videoTrack.height,
            this.mkvHeader.duration * 1000, videoTrack.codec_id
        );

        if (audioTrack) {
            const audioMime = `audio/mp4; codecs="${audioTrack.codec_string}"`;
            const canPlayNatively = MediaSource.isTypeSupported(audioMime);
            const strictlyUnsupported = ["A_TRUEHD", "A_DTS"];

            if (canPlayNatively && !strictlyUnsupported.includes(audioTrack.codec_id)) {
                this.log(`Direct Play Supported! Bypassing Transcoder for: ${audioTrack.codec_id}`);
                this.needsAudioTranscode = false;
                this.demuxer.setTranscodeMode(false);
            } else {
                this.log(`Browser cannot direct-play ${audioTrack.codec_id}. Booting Transcoder`);
                this.needsAudioTranscode = true;
                this.demuxer.setTranscodeMode(true);
                this._bootAudioEncoder();
            }
        }

        this.videoTrack = videoTrack;
        this.audioTrack = audioTrack;

        const MSE = window.ManagedMediaSource || window.MediaSource;
        if (!MSE) return this.log("Error: MSE not supported.");

        this.mediaSource = new MSE();
        this.mediaSource.addEventListener('sourceopen', () => this._onSourceOpen());
    }

    async _onSourceOpen() {
        try {
            let mime = `video/mp4; codecs="${this.videoTrack.codec_string}`;
            if (this.audioTrack) {
                mime += this.needsAudioTranscode ? `, mp4a.40.2"` : `, ${this.audioTrack.codec_string}"`;
            } else { mime += `"`; }

            this.sourceBuffer = this.mediaSource.addSourceBuffer(mime);
            this.sourceBuffer.mode = 'segments';
            this.mediaSource.duration = this.mkvHeader.duration;

            await new Promise(r => setTimeout(r, 100));

            const initData = this.demuxer.init(this.initialHeaderData);

            this.mp4InitSegment = initData;

            if (!initData || initData.length < 100) throw new Error("Invalid Init Segment from Rust");
            await this._appendToBuffer(initData);

            this.currentOffset = this.firstClusterOffset || 0;
            this.log("▶️ Stream routed to screen. Buffering clusters...");
            this._streamLoop();

        } catch (error) {
            console.error("Engine Crash in _onSourceOpen:", error);
        }
    }

    async _streamLoop() {
        if (!this.sourceBuffer || this.mediaSource.readyState !== 'open') return;

        let myStreamId = this.currentStreamId;
        if (this.isFetching || this.currentOffset >= this.sourceInput.size) return;
        this.isFetching = true;

        while (this.currentOffset < this.sourceInput.size && myStreamId === this.currentStreamId) {
            let bufferedEnd = this.video ? this.video.currentTime : 0;
            for (let i = 0; i < this.sourceBuffer.buffered.length; i++) {
                let end = this.sourceBuffer.buffered.end(i);
                if (end > bufferedEnd) bufferedEnd = end;
            }

            let limit = this.video ? (bufferedEnd - this.video.currentTime) : bufferedEnd;
            if (limit > 30) break;

            this.abortController = new AbortController();
            try {
                let bytesProcessed = 0;

                for await (const chunkData of this.sourceInput.stream(this.currentOffset, this.currentOffset + this.chunkSize, this.abortController.signal)) {
                    if (myStreamId !== this.currentStreamId) break;

                    const isFinal = (this.currentOffset + bytesProcessed + chunkData.length) >= this.sourceInput.size;
                    const framesStaged = this.demuxer.parse_chunk(chunkData, isFinal);

                    if (this.audioTrack && this.needsAudioTranscode && this.audioEncoder?.state === 'configured') {
                        while (true) {
                            const samples = wasm._demuxer_decode_next_audio_frame(this.demuxer.ptr);
                            if (samples <= 0) break;

                            const pcmPtr = wasm._get_audio_ptr();
                            const dtsBigInt = wasm._demuxer_get_last_audio_dts(this.demuxer.ptr);

                            const leftChannel = new Float32Array(getWasmMemory(), pcmPtr, samples).slice();
                            const rightChannel = new Float32Array(getWasmMemory(), pcmPtr + 768000, samples).slice();

                            const audioData = new AudioData({
                                format: 'f32-planar',
                                sampleRate: 48000,
                                numberOfChannels: 2,
                                numberOfFrames: samples,
                                timestamp: (Number(dtsBigInt) * 1000000) / 48000,
                                data: new Float32Array([...leftChannel, ...rightChannel])
                            });

                            this.audioEncoder.encode(audioData);
                            audioData.close();
                        }
                        await new Promise(r => setTimeout(r, 1));
                    }

                    if (framesStaged >= 30 || isFinal) {
                        const segment = this.demuxer.get_mp4_segment();

                        if (this.isRecording && segment.length > 0) {
                            this.downloadBuffer.push(new Uint8Array(segment));
                        }

                        if (segment.length > 0 && this.sourceBuffer) {
                            await this._appendToBuffer(segment);

                            // THE STARTUP NUDGE FIX
                            if (this.video && this.video.currentTime === 0 && this.sourceBuffer.buffered.length > 0) {
                                const start = this.sourceBuffer.buffered.start(0);
                                if (start > 0 && start < 0.5) {
                                    this.video.currentTime = start + 0.01;
                                }
                            }

                            await new Promise(r => requestAnimationFrame(r));
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

    _onTimeUpdate() {
        if (!this.sourceBuffer || !this.video || this.mediaSource.readyState !== 'open') return;
        if (!this.video.paused && this.video.readyState <= 2) {
            for (let i = 0; i < this.sourceBuffer.buffered.length; i++) {
                let start = this.sourceBuffer.buffered.start(i);
                if (start > this.video.currentTime && start - this.video.currentTime < 0.5) {
                    this.video.currentTime = start + 0.01; break;
                }
            }
        }
        this._streamLoop();
    }

    async _onSeeking() {
        if (!this.video || !this.cueMap || this.cueMap.length === 0 || !this.mediaSource || this.mediaSource.readyState !== 'open') return;
        if (this.abortController) { this.abortController.abort(); this.abortController = null; }
        if (this.isSeeking) return;

        this.isSeeking = true;
        this.currentStreamId++;

        try {
            if (this.sourceBuffer) {
                if (this.sourceBuffer.updating) this.sourceBuffer.abort();
                if (this.sourceBuffer.buffered.length > 0) {
                    const wipeStart = Math.max(0, this.video.currentTime - 1);
                    this.sourceBuffer.remove(wipeStart, this.mediaSource.duration);
                    await new Promise(r => this.sourceBuffer.addEventListener('updateend', r, { once: true }));
                }
            }
        } catch (e) { }

        if (this.demuxer) this.demuxer.reset();
        if (this.audioTrack) this._bootAudioEncoder();

        let bestCue = this.cueMap[0];
        for (let i = 0; i < this.cueMap.length; i++) {
            if (this.cueMap[i].time <= this.video.currentTime) bestCue = this.cueMap[i];
            else break;
        }

        const segmentPayloadStart = this.firstClusterOffset - Number(this.cueMap[0].offset);
        this.currentOffset = segmentPayloadStart + Number(bestCue.offset);

        this.isFetching = false;
        this.isSeeking = false;
        this._streamLoop();
        try { await this.video.play(); } catch (e) { }
    }

    async switchAudioTrack(newTrackNumber) {
        if (!this.video) return;
        const targetTime = this.video.currentTime;
        this.video.pause();

        if (this.abortController) { this.abortController.abort(); this.abortController = null; }
        if (this.isSeeking) return;
        this.isSeeking = true;
        this.currentStreamId++;

        const newAudioTrack = this.audioTracks.find(t => t.track_number === Number(newTrackNumber));
        if (!newAudioTrack) return;
        this.audioTrack = newAudioTrack;

        try {
            if (this.sourceBuffer) {
                if (this.sourceBuffer.updating) this.sourceBuffer.abort();
                if (this.sourceBuffer.buffered.length > 0) {
                    this.sourceBuffer.remove(0, this.mediaSource.duration);
                    await new Promise(r => this.sourceBuffer.addEventListener('updateend', r, { once: true }));
                }
            }
        } catch (e) { }

        if (this.demuxer) this.demuxer.destroy();
        this.demuxer = new Demuxer(
            BigInt(this.videoTrack.track_number), BigInt(newAudioTrack.track_number),
            this.videoTrack.width, this.videoTrack.height,
            this.mkvHeader.duration * 1000, this.videoTrack.codec_id
        );

        if (this.audioTrack) {
            const audioMime = `audio/mp4; codecs="${this.audioTrack.codec_string}"`;
            const canPlayNatively = MediaSource.isTypeSupported(audioMime);

            // Only blacklist DTS and TrueHD
            const strictlyUnsupported = ["A_TRUEHD", "A_DTS"];

            if (canPlayNatively && !strictlyUnsupported.includes(this.audioTrack.codec_id)) {
                this.needsAudioTranscode = false;
                this.demuxer.setTranscodeMode(false);

                if (this.audioEncoder && this.audioEncoder.state !== 'closed') {
                    try { this.audioEncoder.close(); } catch (e) { }
                }
            } else {
                this.needsAudioTranscode = true;
                this.demuxer.setTranscodeMode(true);
                this._bootAudioEncoder();
            }
        }

        const newInitSegment = this.demuxer.init(this.initialHeaderData);
        await this._appendToBuffer(newInitSegment);

        let bestCue = this.cueMap[0];
        for (let i = 0; i < this.cueMap.length; i++) {
            if (this.cueMap[i].time <= targetTime) bestCue = this.cueMap[i];
            else break;
        }

        const segmentPayloadStart = this.firstClusterOffset - Number(this.cueMap[0].offset);
        this.currentOffset = segmentPayloadStart + Number(bestCue.offset);

        if (bestCue && Math.abs(this.video.currentTime - bestCue.time) > 0.2) {
            this.video.currentTime = bestCue.time;
        }

        this.isFetching = false;
        this.isSeeking = false;
        this._streamLoop();
        try { await this.video.play(); } catch (e) { }
    }

    async _appendToBuffer(data) {
        return new Promise((resolve, reject) => {
            if (!this.sourceBuffer) return resolve();
            if (this.sourceBuffer.updating) {
                setTimeout(() => this._appendToBuffer(data).then(resolve).catch(reject), 50);
                return;
            }
            try {
                const onUpdate = () => { cleanup(); resolve(); };
                const onError = (e) => { cleanup(); reject(e); };
                const cleanup = () => {
                    this.sourceBuffer.removeEventListener('updateend', onUpdate);
                    this.sourceBuffer.removeEventListener('error', onError);
                };
                this.sourceBuffer.addEventListener('updateend', onUpdate);
                this.sourceBuffer.addEventListener('error', onError);
                this.sourceBuffer.appendBuffer(data);
            } catch (e) { reject(e); }
        });
    }
}

// --- EXPORTED SDK FUNCTIONS ---
const streamDictionary = new Map();

export async function feed(source) {
    if (!wasm) { wasm = await initModule(); }
    const dictKey = source instanceof File ? source.name : source;

    if (streamDictionary.has(dictKey)) return streamDictionary.get(dictKey);

    const fetcher = new MKVFetcher(source);
    await fetcher.init();

    const engine = new CoreEngine();
    await engine.preload(fetcher);

    streamDictionary.set(dictKey, engine);
    return engine;
}

export class MKVPlayer {
    constructor(videoElement) {
        if (!videoElement) throw new Error("MKVPlayer requires a <video> element!");
        this.video = videoElement;
        this.engine = null;
    }

    async load(source) {
        // Completely reset the video tag
        if (this.video) {
            this.video.pause();
            this.video.currentTime = 0; // Reset the timeline
            this.video.removeAttribute('src'); // Detach the old SourceBuffer
            this.video.load(); // Force the browser to flush its video memory
        }

        const isFile = source instanceof File;
        const dictKey = isFile ? source.name : source;

        // 2. Load the new stream
        if (!streamDictionary.has(dictKey)) {
            await feed(source);
        }

        // 3. Attach the fresh engine
        this.engine = streamDictionary.get(dictKey);
        this.engine.attachVideo(this.video);
    }

    play() {
        this.video.play().catch(e => {
            if (e.name !== 'AbortError') console.error("Play prevented:", e);
        });
    }

    pause() { this.video.pause(); }
    seek(timeInSeconds) { this.video.currentTime = timeInSeconds; }
    getAudioTracks() { return this.engine ? this.engine.audioTracks : []; }
    setAudioTrack(trackNumber) { if (this.engine) this.engine.switchAudioTrack(trackNumber); }

    toggleRecording(buttonElement) {
        if (!this.engine) return;

        if (!this.engine.isRecording) {
            this.engine.isRecording = true;
            this.engine.downloadBuffer = [];
            if (this.engine.mp4InitSegment) {
                this.engine.downloadBuffer.push(new Uint8Array(this.engine.mp4InitSegment));
            }
            buttonElement.innerText = "Stop & Save MP4";
            buttonElement.style.background = "#ff4444";
        } else {
            this.engine.isRecording = false;
            buttonElement.innerText = "⬇Save MP4 to Device";
            buttonElement.style.background = "#28a745";

            const blob = new Blob(this.engine.downloadBuffer, { type: 'video/mp4' });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.style.display = 'none';
            a.href = url;
            a.download = 'Transcoded_Movie.mp4';

            document.body.appendChild(a);
            a.click();

            window.URL.revokeObjectURL(url);
            document.body.removeChild(a);
            this.engine.downloadBuffer = [];
        }
    }
}