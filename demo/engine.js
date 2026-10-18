// Thin wrapper over the reactive-vcr-wasm C ABI (see crates/reactive-vcr-wasm/src/lib.rs).

const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function instantiate(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`${url} returned HTTP ${response.status}`);
  }
  if (WebAssembly.instantiateStreaming) {
    try {
      return (await WebAssembly.instantiateStreaming(response.clone(), {})).instance;
    } catch {
      // Served without the application/wasm MIME type; compile from bytes instead.
    }
  }
  return (await WebAssembly.instantiate(await response.arrayBuffer(), {})).instance;
}

export async function loadEngine(url) {
  const x = (await instantiate(url)).exports;
  const bytes = (ptr, len) => new Uint8Array(x.memory.buffer, ptr, len);
  const readText = (len) => decoder.decode(bytes(x.rvcr_text_ptr(), len));
  const writeText = (text) => {
    const data = encoder.encode(text);
    bytes(x.rvcr_text_buffer(data.length), data.length).set(data);
    return data.length;
  };
  // Non-negative results pass through; `-n` carries an n-byte error message.
  const check = (status) => {
    if (status < 0) throw new Error(readText(-status));
    return status;
  };

  return {
    /** Preset paths the mapping reads, as written in its TOML. */
    mappingFiles(toml) {
      return JSON.parse(readText(check(x.rvcr_mapping_files(writeText(toml)))));
    },

    /** Compiles a mapping; `files` maps each preset path to its text. Throws on errors. */
    setMapping(toml, files) {
      check(x.rvcr_set_mapping(writeText(JSON.stringify({ toml, files }))));
    },

    /** Mono samples at 48 kHz that drive the effect. */
    setAudio(samples) {
      // Allocate first: growing linear memory detaches earlier views of it.
      const ptr = x.rvcr_audio_buffer(samples.length);
      new Float32Array(x.memory.buffer, ptr, samples.length).set(samples);
    },

    /** Analyzes the audio and schedules every frame; returns the frame count. */
    build(fps) {
      return check(x.rvcr_build(fps));
    },

    /** Renders an RGBA frame with the settings scheduled for `frame`; returns a view of the result. */
    process(rgba, width, height, frame) {
      const len = width * height * 4;
      bytes(x.rvcr_frame_buffer(width, height), len).set(rgba);
      const status = x.rvcr_process(width, height, frame);
      if (status !== 0) {
        throw new Error(status === 2 ? "no schedule built" : "frame buffer size mismatch");
      }
      return new Uint8ClampedArray(x.memory.buffer, x.rvcr_output(), len);
    },

    /** Forgets temporal effect state (after a seek, loop, or source change). */
    reset() {
      x.rvcr_reset();
    },
  };
}
