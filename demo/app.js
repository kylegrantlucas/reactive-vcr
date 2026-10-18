const videos = [...document.querySelectorAll("video[data-src]")];

function pauseOtherVideos(active) {
  for (const video of videos) {
    if (video !== active) video.pause();
  }
}

async function loadVideo(button) {
  const video = document.getElementById(button.dataset.for);
  if (!video) return;
  pauseOtherVideos(video);
  if (!video.src) {
    video.src = video.dataset.src;
    video.load();
  }
  button.hidden = true;
  try {
    await video.play();
  } catch {
    button.hidden = false;
  }
}

for (const button of document.querySelectorAll(".load-video")) {
  button.addEventListener("click", () => loadVideo(button));
}

for (const video of videos) {
  video.addEventListener("play", () => pauseOtherVideos(video));
}

for (const copyButton of document.querySelectorAll(".copy-command")) {
  copyButton.addEventListener("click", async () => {
    const command = copyButton.previousElementSibling?.textContent?.trim();
    if (!command) return;
    await navigator.clipboard.writeText(command);
    copyButton.textContent = "Copied";
    window.setTimeout(() => { copyButton.textContent = "Copy"; }, 1400);
  });
}
