// ---------------------------------------------------------------------------
// Experiment Web Worker — runs WASM experiments off the main thread.
//
// Loads the same mafis_bg.wasm binary but instantiates it manually,
// skipping __wbindgen_start (which runs Bevy's fn main and needs DOM/Canvas).
// Only the experiment functions (experiment_start, experiment_run_single,
// experiment_finish) are called — they use thread_local storage and never
// touch the ECS or rendering.
//
// Import stubs are auto-generated from WebAssembly.Module.imports(), with
// real implementations for the functions the experiment code path calls.
// Uses the same externref table pattern as the generated wasm-bindgen JS.
// ---------------------------------------------------------------------------

let wasm = null;
let ready = false;
let cancelled = false;

// -- String marshaling (from wasm-bindgen generated code) --------------------

let WASM_VECTOR_LEN = 0;
let cachedUint8Mem = null;
let cachedDataView = null;

const textEncoder = new TextEncoder();
let textDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
textDecoder.decode();

function getU8() {
    if (cachedUint8Mem === null || cachedUint8Mem.byteLength === 0)
        cachedUint8Mem = new Uint8Array(wasm.memory.buffer);
    return cachedUint8Mem;
}

function getDV() {
    if (cachedDataView === null || cachedDataView.buffer.detached === true
        || (cachedDataView.buffer.detached === undefined && cachedDataView.buffer !== wasm.memory.buffer))
        cachedDataView = new DataView(wasm.memory.buffer);
    return cachedDataView;
}

function passString(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = textEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getU8().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }
    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;
    const mem = getU8();
    let offset = 0;
    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) arg = arg.slice(offset);
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getU8().subarray(ptr + offset, ptr + len);
        const ret = textEncoder.encodeInto(arg, view);
        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }
    WASM_VECTOR_LEN = offset;
    return ptr;
}

function getString(ptr, len) {
    ptr = ptr >>> 0;
    return textDecoder.decode(getU8().subarray(ptr, ptr + len));
}

function getArrayU8(ptr, len) {
    ptr = ptr >>> 0;
    return getU8().subarray(ptr, ptr + len);
}

function isLikeNone(x) { return x === undefined || x === null; }

// -- Heap object table (mirrors wasm-bindgen's addHeapObject/getObject) --------
// The WASM binary uses integer indices into a JS-side heap array to pass
// opaque JS objects (globalThis, Performance, etc.) across the WASM boundary.

let heap = new Array(128).fill(undefined);
heap.push(undefined, null, true, false);
let heap_next = heap.length;

function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];
    heap[idx] = obj;
    return idx;
}

function getObject(idx) { return heap[idx]; }

function dropObject(idx) {
    if (idx < 132) return;
    heap[idx] = heap_next;
    heap_next = idx;
}

function handleError(f, args) {
    try {
        return f.apply(null, args);
    } catch (e) {
        wasm.__wbindgen_export3(addHeapObject(e));
    }
}

// -- Build import object from module introspection ---------------------------

function buildImports(module) {
    const required = WebAssembly.Module.imports(module);

    // Group by module name, auto-stub everything as no-op returning undefined
    const modules = {};
    for (const imp of required) {
        if (!modules[imp.module]) modules[imp.module] = {};
        if (imp.kind === 'function') {
            modules[imp.module][imp.name] = () => {};
        }
    }

    // The single import module for wasm-bindgen
    const bg = modules["./mafis_bg.js"];
    if (!bg) throw new Error("Missing wasm-bindgen import module");

    // -- Override stubs using name pattern matching --
    // wasm-bindgen names are: __wbg_<identifier>_<hex_hash>
    // We match by the identifier part, ignoring the hash suffix.

    for (const name of Object.keys(bg)) {
        // --- wasm-bindgen internals ---

        if (name === '__wbindgen_init_externref_table') {
            // No-op: this binary uses heap-based object passing, not externref
            bg[name] = () => {};
        }
        else if (name === '__wbindgen_object_drop_ref') {
            bg[name] = (arg0) => { dropObject(arg0); };
        }
        else if (name === '__wbindgen_object_clone_ref') {
            bg[name] = (arg0) => {
                const ret = getObject(arg0);
                return addHeapObject(ret);
            };
        }
        else if (name.includes('__wbindgen_throw')) {
            bg[name] = (ptr, len) => { throw new Error(getString(ptr, len)); };
        }
        else if (name.includes('__wbindgen_boolean_get')) {
            bg[name] = (v) => typeof v === 'boolean' ? (v ? 1 : 0) : 0xFFFFFF;
        }
        else if (name.includes('__wbindgen_is_null')) {
            bg[name] = (v) => v === null;
        }
        else if (name.includes('__wbindgen_is_undefined')) {
            bg[name] = (v) => v === undefined;
        }
        else if (name.includes('__wbindgen_is_function')) {
            bg[name] = (v) => typeof v === 'function';
        }
        else if (name.includes('__wbindgen_number_get')) {
            bg[name] = (arg0, arg1) => {
                const ret = typeof arg1 === 'number' ? arg1 : undefined;
                const none = isLikeNone(ret);
                getDV().setFloat64(arg0 + 8, none ? 0 : ret, true);
                getDV().setInt32(arg0, !none, true);
            };
        }
        else if (name.includes('__wbindgen_string_get')) {
            bg[name] = (arg0, arg1) => {
                const ret = typeof arg1 === 'string' ? arg1 : undefined;
                const none = isLikeNone(ret);
                const ptr1 = none ? 0 : passString(ret, wasmMalloc(), wasmRealloc());
                const len1 = WASM_VECTOR_LEN;
                getDV().setInt32(arg0 + 4, len1, true);
                getDV().setInt32(arg0, ptr1, true);
            };
        }
        else if (name.includes('__wbindgen_debug_string')) {
            bg[name] = (arg0, arg1) => {
                const ret = String(arg1);
                const ptr1 = passString(ret, wasmMalloc(), wasmRealloc());
                const len1 = WASM_VECTOR_LEN;
                getDV().setInt32(arg0 + 4, len1, true);
                getDV().setInt32(arg0, ptr1, true);
            };
        }

        // --- Static accessors (globalThis, self, window, global) ---
        // These use addToExternrefTable0 to store the object and return an index.

        else if (name.includes('static_accessor_SELF')) {
            bg[name] = () => {
                const ret = typeof self === 'undefined' ? null : self;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            };
        }
        else if (name.includes('static_accessor_WINDOW')) {
            bg[name] = () => 0; // no window in Worker
        }
        else if (name.includes('static_accessor_GLOBAL_THIS')) {
            bg[name] = () => {
                const ret = typeof globalThis === 'undefined' ? null : globalThis;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            };
        }
        else if (name.includes('static_accessor_GLOBAL') && !name.includes('GLOBAL_THIS')) {
            bg[name] = () => {
                const ret = typeof global === 'undefined' ? null : global;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            };
        }

        // --- Property accessors used by web-time and getrandom ---
        // These receive heap indices and must dereference via getObject().

        else if (name.includes('_performance_') && !name.includes('observe')) {
            bg[name] = (arg0) => {
                const ret = getObject(arg0).performance;
                return addHeapObject(ret);
            };
        }
        else if (name.includes('_now_') && !name.includes('amount')) {
            bg[name] = function() {
                if (arguments.length === 0) return Date.now();
                return getObject(arguments[0]).now();
            };
        }
        else if (name.includes('_crypto_') && !name.includes('subtle')) {
            bg[name] = (arg0) => getObject(arg0).crypto;
        }
        else if (name.includes('getRandomValues')) {
            bg[name] = function() {
                return handleError(function(arg0, arg1) {
                    globalThis.crypto.getRandomValues(getArrayU8(arg0, arg1));
                }, arguments);
            };
        }

        // --- Uint8Array / ArrayBuffer operations ---

        else if (name.includes('_newwithbyteoffsetandlength_')) {
            bg[name] = (buf, offset, len) => new Uint8Array(buf, offset >>> 0, len >>> 0);
        }
        else if (name.includes('_instanceof_Uint8Array_')) {
            bg[name] = (arg0) => arg0 instanceof Uint8Array;
        }
        else if (name.includes('_instanceof_ArrayBuffer_')) {
            bg[name] = (arg0) => arg0 instanceof ArrayBuffer;
        }

        // --- Closures callback ref (never used in experiments, safe no-op) ---

        else if (name.includes('_wbg_cb_unref_')) {
            bg[name] = () => {};
        }

        // --- Error suppression for eprintln! / console output ---

        else if (name.includes('_error_') && name.startsWith('__wbg_error')) {
            bg[name] = () => {};
        }
        else if (name.includes('_log_') && name.startsWith('__wbg_log')) {
            bg[name] = () => {};
        }
        else if (name.includes('_warn_') && name.startsWith('__wbg_warn')) {
            bg[name] = () => {};
        }
    }

    return modules;
}

// -- Experiment wrappers -----------------------------------------------------

// Resolve malloc/realloc/free exports — wasm-bindgen uses either named
// (__wbindgen_malloc) or numbered (__wbindgen_export) exports depending
// on strip settings.  Detect both so the worker survives either config.
function wasmMalloc() {
    return wasm.__wbindgen_malloc || wasm.__wbindgen_export;
}
function wasmRealloc() {
    return wasm.__wbindgen_realloc || wasm.__wbindgen_export2;
}
function wasmFree() {
    return wasm.__wbindgen_free || wasm.__wbindgen_export4;
}

function experimentStart() {
    wasm.experiment_start();
}

function experimentRunSingle(configJson) {
    let deferred0;
    let deferred1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        const ptr = passString(configJson, wasmMalloc(), wasmRealloc());
        const len = WASM_VECTOR_LEN;
        wasm.experiment_run_single(retptr, ptr, len);
        const r0 = getDV().getInt32(retptr + 4 * 0, true);
        const r1 = getDV().getInt32(retptr + 4 * 1, true);
        deferred0 = r0;
        deferred1 = r1;
        return getString(r0, r1);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasmFree()(deferred0, deferred1, 1);
    }
}

function experimentFinish() {
    let deferred0;
    let deferred1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.experiment_finish(retptr);
        const r0 = getDV().getInt32(retptr + 4 * 0, true);
        const r1 = getDV().getInt32(retptr + 4 * 1, true);
        deferred0 = r0;
        deferred1 = r1;
        return getString(r0, r1);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasmFree()(deferred0, deferred1, 1);
    }
}

// -- Worker message handler --------------------------------------------------

self.onmessage = async function(e) {
    const msg = e.data;

    switch (msg.type) {
        case 'init': {
            try {
                // Accept pre-compiled module (multi-worker) or fetch/compile from URL.
                const module = (msg.module instanceof WebAssembly.Module)
                    ? msg.module
                    : await WebAssembly.compileStreaming(fetch(msg.wasmUrl || 'mafis_bg.wasm'));
                const imports = buildImports(module);
                const instance = await WebAssembly.instantiate(module, imports);
                wasm = instance.exports;

                // Reset memory caches now that wasm is set
                cachedUint8Mem = null;
                cachedDataView = null;

                // Initialize the externref table. In the normal flow this
                // happens inside __wbindgen_start, which we skip.
                // The __wbindgen_init_externref_table import stub (above)
                // handles this, but __wbindgen_start is the one that calls
                // it. So we call the WASM export directly if it exists,
                // otherwise do it manually.
                const table = wasm.__wbindgen_externrefs;
                if (table) {
                    const offset = table.grow(4);
                    table.set(0, undefined);
                    table.set(offset + 0, undefined);
                    table.set(offset + 1, null);
                    table.set(offset + 2, true);
                    table.set(offset + 3, false);
                }

                ready = true;
                self.postMessage({ type: 'ready' });
            } catch (err) {
                self.postMessage({ type: 'error', message: `WASM init failed: ${err.message}` });
            }
            break;
        }

        case 'runAll': {
            if (!ready) {
                self.postMessage({ type: 'error', message: 'Worker not initialized' });
                return;
            }

            cancelled = false;
            const configs = msg.configs;
            const total = configs.length;

            try {
                experimentStart();

                for (let i = 0; i < total; i++) {
                    if (cancelled) {
                        self.postMessage({ type: 'cancelled', index: i });
                        break;
                    }
                    const brief = experimentRunSingle(JSON.stringify(configs[i]));
                    self.postMessage({ type: 'progress', index: i, total, brief });
                }

                const resultJson = experimentFinish();
                if (!cancelled) {
                    self.postMessage({ type: 'done', json: resultJson });
                } else {
                    self.postMessage({ type: 'done', json: resultJson, partial: true });
                }
            } catch (err) {
                self.postMessage({ type: 'error', message: `Run failed: ${err.message}` });
            }
            break;
        }

        // runOne: single-config dispatch for work-stealing pool.
        // Calls start→run_single→finish for exactly one config, then posts runOneDone.
        // The caller is responsible for dispatching the next job on receipt.
        case 'runOne': {
            if (!ready) {
                self.postMessage({ type: 'error', message: 'Worker not initialized' });
                return;
            }
            try {
                experimentStart();
                const brief = experimentRunSingle(JSON.stringify(msg.config));
                const resultJson = experimentFinish();
                self.postMessage({ type: 'runOneDone', index: msg.index, brief, json: resultJson });
            } catch (err) {
                self.postMessage({ type: 'error', message: `runOne failed: ${err.message}` });
            }
            break;
        }

        case 'cancel': {
            cancelled = true;
            break;
        }
    }
};
