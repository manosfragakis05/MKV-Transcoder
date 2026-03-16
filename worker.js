import { feed } from './mkv_lib.js';

let engine = null;

self.onmessage = async function (e) {
    const { type, payload } = e.data;

    switch (type) {
        case 'INIT':
            console.log("👷‍♂️ Worker: Booting WASM Engine...");
            engine = await feed(payload.url);
            
            // Wire up the Zero-Copy Transferables
            engine.onSegmentReady = (bytes) => self.postMessage({ type: 'APPEND', data: bytes }, [bytes.buffer]);
            engine.onAudioReady = (pcm) => self.postMessage({ type: 'PCM_DATA', data: pcm }, [pcm.buffer]);
            engine.onAudioSwap = (init, codec) => self.postMessage({ type: 'AUDIO_SWAP', initSegment: init, codecString: codec }, [init.buffer]);

            self.postMessage({
                type: 'READY',
                audioTracks: engine.audioTracks,
                videoTrack: engine.videoTrack,
                duration: engine.mkvHeader.duration
            });
            break;

        case 'START':
            if (engine) engine.start(payload.needsFfmpeg);
            break;

        case 'SYNC':
            if (engine) engine.syncProgress(payload.currentTime, payload.bufferedEnd, payload.queueLength);
            break;

        case 'SEEK':
            if (engine) engine.seekTo(payload.time);
            break;

        case 'SWITCH_AUDIO':
            if (engine) engine.switchAudioTrack(payload.trackId, payload.currentTime);
            break;

        default:
            console.warn("👷‍♂️ Worker: Unknown command received ->", type);
    }
};