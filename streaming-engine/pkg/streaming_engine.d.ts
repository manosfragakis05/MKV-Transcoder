/* tslint:disable */
/* eslint-disable */

export class Demuxer {
    free(): void;
    [Symbol.dispose](): void;
    get_mp4_segment(): Uint8Array;
    init(chunk: Uint8Array): Uint8Array;
    constructor(video_track_id: bigint, audio_track_id: bigint, width: number, height: number, _duration_ms: number, codec_id: string);
    parse_chunk(chunk: Uint8Array, is_final: boolean): number;
    reset(): void;
}

export function alloc_memory(size: number): number;

export function free_memory(ptr: number, size: number): void;

export function get_mkv_info(mkv_data: Uint8Array): any;

export function get_mkv_info_fast(ptr: number, len: number): any;

export function init_hooks(): void;

export function parse_cues(cues_data: Uint8Array): any;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_demuxer_free: (a: number, b: number) => void;
    readonly alloc_memory: (a: number) => number;
    readonly demuxer_get_mp4_segment: (a: number) => [number, number, number, number];
    readonly demuxer_init: (a: number, b: number, c: number) => [number, number, number, number];
    readonly demuxer_new: (a: bigint, b: bigint, c: number, d: number, e: number, f: number, g: number) => number;
    readonly demuxer_parse_chunk: (a: number, b: number, c: number, d: number) => number;
    readonly demuxer_reset: (a: number) => void;
    readonly free_memory: (a: number, b: number) => void;
    readonly get_mkv_info: (a: number, b: number) => [number, number, number];
    readonly get_mkv_info_fast: (a: number, b: number) => [number, number, number];
    readonly init_hooks: () => void;
    readonly parse_cues: (a: number, b: number) => [number, number, number];
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
