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
    readonly experiment_finish: (a: number) => void;
    readonly experiment_run_single: (a: number, b: number, c: number) => void;
    readonly experiment_start: () => void;
    readonly get_simulation_state: (a: number) => void;
    readonly send_command: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_83209: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_79912: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_72361: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_72350: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_83211: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_2: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_3: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_4: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_5: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_6: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_7: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83211_8: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83210: (a: number, b: number, c: number) => void;
    readonly __wasm_bindgen_func_elem_83208: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number, c: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
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
