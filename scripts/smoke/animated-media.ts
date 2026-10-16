// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { modelConfigurationSchema } from "../../apps/nous-core/src/model/configuration.js";
import { sampleVideo } from "../../apps/nous-core/src/model/video.js";
import { workspaceTemp } from "../workspace.js";
const home = await workspaceTemp("smoke", "animated-media-");
const ffmpeg = process.env.NOUS_MEDIA_FFMPEG;
if (!ffmpeg)
  throw new Error(
    "Set NOUS_MEDIA_FFMPEG to the installed native FFmpeg component",
  );
execFileSync("python3", [
  "-c",
  `from PIL import Image
import sys
from pathlib import Path
p=Path(sys.argv[1])
f=[Image.new('RGB',(64,48),c) for c in ['red','green','blue','yellow']]
f[0].save(p/'timeline.gif',save_all=True,append_images=f[1:],duration=[500]*4,loop=0)
f[0].save(p/'static.png')`,
  home,
]);
const policy = modelConfigurationSchema.parse({
  video: { ffmpeg_executable: ffmpeg, max_frames: 4 },
}).video;
const bytes = await readFile(join(home, "timeline.gif"));
const full = await sampleVideo(
  bytes,
  policy,
  home,
  [],
  false,
  undefined,
  "image/gif",
);
assert.equal(full.quality.source_duration_seconds, 2);
assert.equal(full.quality.frame_coverage, "sampled");
assert.equal(full.quality.truncated, false);
assert(full.frames.length >= 3);
for (const [index, frame] of full.frames.entries())
  await writeFile(join(home, `frame-${index}.jpg`), frame.bytes);
const colors = JSON.parse(
  execFileSync(
    "python3",
    [
      "-c",
      `from PIL import Image
import sys,json
from pathlib import Path
print(json.dumps([Image.open(p).convert('RGB').getpixel((32,24)) for p in sorted(Path(sys.argv[1]).glob('frame-*.jpg'))]))`,
      home,
    ],
    { encoding: "utf8" },
  ),
) as number[][];
assert(
  new Set(colors.map((c) => c.join(","))).size >= 3,
  "GIF must expose distinct actual timeline frames",
);
const partial = await sampleVideo(
  bytes,
  { ...policy, max_video_seconds: 1 },
  home,
  [],
  false,
  undefined,
  "image/gif",
);
assert.equal(partial.quality.truncated, true);
assert(partial.frames.every((f) => f.timestamp < 1));
execFileSync(
  ffmpeg,
  [
    "-nostdin",
    "-i",
    join(home, "static.png"),
    "-frames:v",
    "1",
    "-pix_fmt",
    "yuvj420p",
    "-y",
    join(home, "png-decoded.jpg"),
  ],
  { stdio: "pipe" },
);
execFileSync(
  ffmpeg,
  [
    "-nostdin",
    "-i",
    join(home, "timeline.gif"),
    "-c:v",
    "mjpeg",
    "-pix_fmt",
    "yuvj420p",
    "-r",
    "2",
    "-t",
    "2",
    "-y",
    join(home, "timeline.mov"),
  ],
  { stdio: "pipe" },
);
const video = await sampleVideo(
  await readFile(join(home, "timeline.mov")),
  policy,
  home,
  [],
  false,
);
assert(video.frames.length >= 3);
assert.equal(video.quality.partial_coverage, true);
assert(
  video.quality.omitted_sample_timestamps?.length,
  "low-frame-rate tail must be an explicit omitted sample",
);
await writeFile(
  join(home, "manifest.json"),
  JSON.stringify(
    {
      ffmpeg: full.quality.ffmpeg_version,
      full: full.quality,
      partial: partial.quality,
      video: video.quality,
      colors,
      modelCalls: 0,
    },
    null,
    2,
  ),
);
console.log(
  JSON.stringify({
    home,
    animatedGif: "PASS",
    nativeVideo: "PASS",
    pngDecoder: "PASS",
    modelCalls: 0,
  }),
);
