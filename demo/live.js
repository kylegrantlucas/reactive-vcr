// Live deck: the reactive-vcr engine compiled to WebAssembly, driven by the page's
// audio clock. The soundtrack is decoded up front and analyzed exactly as `render`
// analyzes it; each animation frame then processes the picture with the settings
// scheduled for the current audio frame.

import { loadEngine } from "./engine.js";

const FPS = 30;
const SAMPLE_RATE = 48000;
const DEMO_MEDIA = "media/00-clean-reference.mp4";
// Resync the picture when it drifts this far from the audio clock (seconds).
const MAX_DRIFT = 0.2;

const $ = (id) => document.getElementById(id);
const ui = {
  frame: $("live-frame"),
  poster: $("live-poster"),
  preview: $("live-preview"),
  start: $("live-start"),
  play: $("live-play"),
  hold: $("live-hold"),
  lines: $("live-lines"),
  label: $("live-label"),
  time: $("live-time"),
  status: $("live-status"),
  videoFile: $("live-video-file"),
  audioFile: $("live-audio-file"),
  mapLink: $("live-map-link"),
  presets: [...document.querySelectorAll(".live-preset")],
  pictureRadios: [...document.querySelectorAll('input[name="live-picture"]')],
  soundRadios: [...document.querySelectorAll('input[name="live-sound"]')],
};

const work = document.createElement("canvas");
const workCtx = work.getContext("2d", { willReadFrequently: true });
const previewCtx = ui.preview.getContext("2d");

const state = {
  engine: null,
  starting: null,
  /** { kind: "demo" | "camera" | "file", video, file?, label, stop() } */
  picture: null,
  /** { kind: "demo" | "file" | "video", audio, label, stop() } */
  sound: null,
  mapped: false,
  slug: null,
  playing: false,
  holding: false,
  lastFrame: -1,
  lastRender: 0,
  dirty: true,
};

// One async operation at a time, so rapid clicks apply in order.
let queue = Promise.resolve();
function enqueue(task) {
  queue = queue.then(task).catch((error) => showStatus(error.message, true));
  return queue;
}

function showStatus(text, isError = false) {
  ui.status.textContent = text;
  ui.status.classList.toggle("error", isError);
}

function checkRadio(radios, value) {
  for (const radio of radios) radio.checked = radio.value === value;
}

function formatTime(seconds) {
  const s = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

async function fetchResponse(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} returned HTTP ${response.status}`);
  return response;
}

// The demo clip is both the demo picture and the demo beat; download it once.
let demoBlob = null;
function demoMedia() {
  demoBlob ??= fetchResponse(DEMO_MEDIA)
    .then((response) => response.blob())
    .catch((error) => {
      demoBlob = null;
      throw error;
    });
  return demoBlob;
}

/* ---- sound ---- */

/** Decodes any browser-supported audio into mono samples at 48 kHz. */
async function decodeMono(data, name) {
  const context = new OfflineAudioContext(1, 1, SAMPLE_RATE);
  let buffer;
  try {
    buffer = await context.decodeAudioData(data);
  } catch {
    throw new Error(`${name} has no sound this browser can decode.`);
  }
  const mono = new Float32Array(buffer.length);
  for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
    const samples = buffer.getChannelData(channel);
    for (let i = 0; i < mono.length; i++) mono[i] += samples[i] / buffer.numberOfChannels;
  }
  return mono;
}

async function openSound(kind, file) {
  const media = kind === "demo" ? await demoMedia() : file;
  const url = URL.createObjectURL(media);
  try {
    const samples = await decodeMono(await media.arrayBuffer(), kind === "demo" ? "The demo beat" : file.name);
    const audio = new Audio();
    audio.loop = true;
    audio.preload = "auto";
    audio.src = url;
    return {
      kind,
      audio,
      samples,
      label: { demo: "the demo beat", video: "its own sound" }[kind] ?? file.name,
      stop() {
        audio.pause();
        audio.removeAttribute("src");
        audio.load();
        URL.revokeObjectURL(url);
      },
    };
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
}

/* ---- picture ---- */

async function loaded(video, name) {
  if (video.readyState >= 2) return;
  await new Promise((resolve, reject) => {
    video.addEventListener("loadeddata", resolve, { once: true });
    video.addEventListener("error", () => reject(new Error(`${name} could not be decoded by this browser.`)), {
      once: true,
    });
  });
}

async function openPicture(kind, file) {
  const video = document.createElement("video");
  video.muted = true;
  video.playsInline = true;
  video.loop = true;
  video.preload = "auto";
  if (kind === "camera") {
    if (!navigator.mediaDevices?.getUserMedia) {
      throw new Error("This browser does not offer camera access on this page.");
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: { ideal: 1280 }, height: { ideal: 720 } },
      audio: false,
    });
    video.srcObject = stream;
    await video.play();
    return {
      kind,
      video,
      label: "your camera",
      stop() {
        stream.getTracks().forEach((track) => track.stop());
        video.srcObject = null;
      },
    };
  }
  const url = URL.createObjectURL(kind === "file" ? file : await demoMedia());
  video.src = url;
  try {
    await loaded(video, kind === "file" ? file.name : "The demo clip");
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
  return {
    kind,
    video,
    file,
    label: kind === "file" ? file.name : "the demo clip",
    stop() {
      video.pause();
      video.removeAttribute("src");
      video.load();
      URL.revokeObjectURL(url);
    },
  };
}

/* ---- engine ---- */

/** Re-analyzes the sound for the current mapping once both are loaded. */
function rebuild() {
  if (!state.mapped || !state.sound) return;
  state.engine.build(FPS);
  state.engine.reset();
  state.dirty = true;
}

async function loadPreset(slug) {
  const button = ui.presets.find((preset) => preset.dataset.slug === slug);
  if (!button) throw new Error(`Unknown preset ${slug}.`);
  const mapUrl = new URL(`maps/${slug}.toml`, document.baseURI);
  const toml = await (await fetchResponse(mapUrl)).text();
  const files = {};
  for (const path of state.engine.mappingFiles(toml)) {
    files[path] = await (await fetchResponse(new URL(path, mapUrl))).text();
  }
  state.engine.setMapping(toml, files);
  state.mapped = true;
  state.slug = slug;
  rebuild();
  for (const preset of ui.presets) preset.setAttribute("aria-pressed", String(preset === button));
  ui.label.textContent = button.querySelector("b").textContent.trim().toUpperCase();
  ui.mapLink.href = `maps/${slug}.toml`;
}

/* ---- sources ---- */

function describe() {
  showStatus(`Playing ${state.picture.label} with ${state.sound.label}.`);
}

async function setSound(kind, file) {
  const sound = await openSound(kind, file);
  state.sound?.stop();
  state.sound = sound;
  state.engine.setAudio(sound.samples);
  rebuild();
  checkRadio(ui.soundRadios, kind);
  state.lastFrame = -1;
  if (state.playing) await startPlayback();
  if (state.picture) describe();
}

async function setPicture(kind, file) {
  const picture = await openPicture(kind, file);
  state.picture?.stop();
  state.picture = picture;
  state.engine.reset();
  state.dirty = true;
  checkRadio(ui.pictureRadios, kind);
  ui.soundRadios.find((radio) => radio.value === "video").disabled = kind !== "file";
  if (state.sound?.kind === "video") {
    // Follow the new video's own sound, or fall back to the demo beat.
    await setSound(kind === "file" ? "video" : "demo", file);
  } else if (state.playing) {
    await startPlayback();
  }
  describe();
}

/* ---- playback ---- */

function pauseShowcaseVideos() {
  for (const video of document.querySelectorAll("video[data-src]")) video.pause();
}

async function startPlayback() {
  pauseShowcaseVideos();
  state.playing = true;
  ui.play.textContent = "Pause";
  try {
    await state.sound.audio.play();
  } catch {
    // Autoplay refused until the visitor interacts; the play button retries.
    pausePlayback();
    showStatus("Press Play to start the sound.");
    return;
  }
  if (state.picture.kind !== "camera") state.picture.video.play().catch(() => {});
}

function pausePlayback() {
  state.playing = false;
  ui.play.textContent = "Play";
  state.sound?.audio.pause();
  if (state.picture?.kind !== "camera") state.picture?.video.pause();
}

/** Keeps a file or demo picture on the audio clock, looping it under longer songs. */
function syncPicture(time) {
  const { video, kind } = state.picture;
  if (kind === "camera" || !Number.isFinite(video.duration) || video.duration <= 0) return;
  const target = time % video.duration;
  const drift = Math.abs(video.currentTime - target);
  if (Math.min(drift, video.duration - drift) > MAX_DRIFT || (!state.playing && drift > 1 / FPS)) {
    video.currentTime = target;
  }
}

function processingSize(video) {
  const lines = Number(ui.lines.value);
  const aspect = video.videoWidth && video.videoHeight ? video.videoWidth / video.videoHeight : 16 / 9;
  return [Math.max(2, Math.round((lines * aspect) / 2) * 2), lines];
}

function render(frame) {
  const { video } = state.picture;
  if (video.readyState < 2) return;
  const [width, height] = processingSize(video);
  if (work.width !== width || work.height !== height) {
    work.width = ui.preview.width = width;
    work.height = ui.preview.height = height;
  }
  workCtx.drawImage(video, 0, 0, width, height);
  if (state.holding) {
    previewCtx.drawImage(work, 0, 0);
    return;
  }
  const pixels = workCtx.getImageData(0, 0, width, height).data;
  const output = state.engine.process(pixels, width, height, frame);
  previewCtx.putImageData(new ImageData(output, width, height), 0, 0);
}

function tick(now) {
  requestAnimationFrame(tick);
  if (!state.picture || !state.sound || !state.mapped) return;
  const time = state.sound.audio.currentTime;
  const frame = Math.floor(time * FPS);
  syncPicture(time);
  // Temporal bends start over when the soundtrack loops.
  if (frame < state.lastFrame) state.engine.reset();
  const cameraDue = state.picture.kind === "camera" && now - state.lastRender >= 1000 / FPS;
  if (frame === state.lastFrame && !state.dirty && !cameraDue) return;
  state.lastFrame = frame;
  state.lastRender = now;
  state.dirty = false;
  ui.time.textContent = formatTime(time);
  try {
    render(frame);
  } catch (error) {
    pausePlayback();
    showStatus(error.message, true);
  }
}

/* ---- start ---- */

async function start(slug) {
  ui.start.disabled = true;
  try {
    showStatus("Loading the engine and decoding the demo beat…");
    state.engine = await loadEngine("reactive-vcr.wasm");
    await setSound("demo");
    await loadPreset(slug ?? ui.presets[0].dataset.slug);
    await setPicture("demo");
  } catch (error) {
    state.engine = null;
    ui.start.disabled = false;
    throw error;
  }
  ui.poster.hidden = true;
  ui.start.hidden = true;
  ui.preview.hidden = false;
  ui.play.disabled = false;
  ui.hold.disabled = false;
  requestAnimationFrame(tick);
  await startPlayback();
}

/** Runs `task` once the deck is started, starting it first if needed. */
function whenStarted(task, slug) {
  return enqueue(async () => {
    if (!state.picture) await start(slug);
    await task();
  });
}

/* ---- wiring ---- */

ui.start.addEventListener("click", () => whenStarted(() => {}));

ui.play.addEventListener("click", () => {
  if (state.playing) pausePlayback();
  else startPlayback();
});

const hold = (on) => {
  state.holding = on && !ui.hold.disabled;
  state.dirty = true;
};
ui.hold.addEventListener("pointerdown", () => hold(true));
for (const type of ["pointerup", "pointerleave", "blur"]) ui.hold.addEventListener(type, () => hold(false));
ui.hold.addEventListener("keydown", (event) => {
  if (event.key === " " || event.key === "Enter") {
    event.preventDefault();
    hold(true);
  }
});
ui.hold.addEventListener("keyup", () => hold(false));

ui.lines.addEventListener("change", () => {
  state.engine?.reset();
  state.dirty = true;
});

for (const button of ui.presets) {
  button.addEventListener("click", () => {
    const slug = button.dataset.slug;
    whenStarted(() => (state.slug === slug ? undefined : loadPreset(slug)), slug);
  });
}

for (const link of document.querySelectorAll("[data-live]")) {
  link.addEventListener("click", () => {
    const slug = link.dataset.live;
    whenStarted(async () => {
      if (state.slug !== slug) await loadPreset(slug);
      if (!state.playing) await startPlayback();
    }, slug);
  });
}

// File radios open a picker and keep the previous choice until a file arrives.
function wireRadios(radios, input, current, choose) {
  for (const radio of radios) {
    radio.addEventListener("change", () => {
      if (radio.value === "file") {
        checkRadio(radios, current());
        input.click();
      } else {
        whenStarted(() => choose(radio.value));
      }
    });
  }
  input.addEventListener("change", () => {
    const [file] = input.files;
    input.value = "";
    if (file) whenStarted(() => choose("file", file));
  });
}

wireRadios(ui.pictureRadios, ui.videoFile, () => state.picture?.kind ?? "demo", setPicture);
wireRadios(
  ui.soundRadios,
  ui.audioFile,
  () => state.sound?.kind ?? "demo",
  (kind, file) => setSound(kind, kind === "video" ? state.picture.file : file),
);

ui.frame.addEventListener("dragover", (event) => {
  event.preventDefault();
  ui.frame.classList.add("dropping");
});
ui.frame.addEventListener("dragleave", () => ui.frame.classList.remove("dropping"));
ui.frame.addEventListener("drop", (event) => {
  event.preventDefault();
  ui.frame.classList.remove("dropping");
  for (const file of event.dataTransfer.files) {
    if (file.type.startsWith("video/")) whenStarted(() => setPicture("file", file));
    else if (file.type.startsWith("audio/")) whenStarted(() => setSound("file", file));
    else showStatus(`${file.name} is not a video or audio file.`, true);
  }
});

// Showcase clips and the live deck take turns with the speakers.
document.addEventListener(
  "play",
  (event) => {
    if (event.target.matches?.("video[data-src]") && state.playing) pausePlayback();
  },
  true,
);
