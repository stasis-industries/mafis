/* tslint:disable */
/* eslint-disable */

export function experiment_finish(): string;

export function experiment_run_single(config_json: string): string;

export function experiment_start(): void;

export function get_simulation_state(): string;

export function send_command(cmd: string): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly main: (a: number, b: number) => number;
    readonly experiment_finish: () => [number, number];
    readonly experiment_run_single: (a: number, b: number) => [number, number];
    readonly experiment_start: () => void;
    readonly get_simulation_state: () => [number, number];
    readonly send_command: (a: number, b: number) => void;
    readonly wasm_bindgen__closure__destroy__h1f80bddcd6d5d5d3: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h310e94b1ca27a306: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h2899a6c619b176ed: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_3: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_4: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_5: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_6: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_7: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_8: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h2e29d3079f3950e2_9: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h8856b7c40a3a02e1: (a: number, b: number, c: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h18d2b746b5616879: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
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
