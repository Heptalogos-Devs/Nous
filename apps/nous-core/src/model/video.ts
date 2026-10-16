// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import type { ModelConfiguration } from "./configuration.js";
import { canonicalDigest } from "../digest.js";

type Policy = ModelConfiguration["video"];
const videoFrameInputIdentity = "labelled-frame-content-v1";
export async function sampleVideo(
  bytes: Uint8Array,
  policy: Policy,
  tempRoot: string,
  secretNames: readonly string[],
  includeAudio: boolean,
  signal?: AbortSignal,
  mediaType?: string,
) {
  const animatedGif = mediaType === "image/gif";
  if (bytes.byteLength > policy.max_source_bytes)
    throw new Error("Video source exceeds configured byte bound");
  const sampling = await videoSampling(policy, secretNames, signal, mediaType);
  const { run, version } = sampling;
  await mkdir(tempRoot, { recursive: true });
  const root = await mkdtemp(join(tempRoot, "nous-video-"));
  try {
    const input = join(root, "source.media");
    await writeFile(input, bytes);
    const inputArgs = [
      "-nostdin",
      "-hide_banner",
      "-max_alloc",
      "67108864",
      "-protocol_whitelist",
      "file,pipe",
      "-format_whitelist",
      animatedGif ? "gif" : "mov,matroska,ogg,avi,mpeg,mpegts",
    ];
    const metadata = await run("metadata", [
      ...inputArgs,
      "-i",
      input,
      "-map",
      "0:v:0",
      "-t",
      "0",
      // Probe stream metadata without depending on a null-output video encoder.
      "-c:v",
      "copy",
      "-f",
      "null",
      "-",
    ]);
    const match = /Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)/.exec(
      metadata.stderr,
    );
    if (!match) throw new Error("Video duration is unavailable");
    const duration =
      Number(match[1]) * 3600 + Number(match[2]) * 60 + Number(match[3]);
    if (!Number.isFinite(duration) || duration <= 0)
      throw new Error("Invalid video duration");
    const bounded = Math.min(duration, policy.max_video_seconds);
    const frames: { bytes: Uint8Array; timestamp: number }[] = [];
    const requested: number[] = [];
    const omitted: number[] = [];
    for (let ordinal = 0; ordinal < policy.max_frames; ordinal++) {
      const at =
        policy.max_frames === 1
          ? 0
          : animatedGif
            ? (ordinal * bounded) / policy.max_frames
            : (ordinal *
                Math.max(0, bounded - policy.frame_end_margin_seconds)) /
              (policy.max_frames - 1);
      requested.push(at);
      const output = join(root, `frame-${ordinal}.jpg`);
      const info = await run("frame", [
        ...inputArgs,
        ...(!animatedGif ? ["-ss", String(at)] : []),
        "-i",
        input,
        "-map",
        "0:v:0",
        "-an",
        "-frames:v",
        "1",
        "-threads",
        "1",
        "-vf",
        `${animatedGif ? `trim=start=${at},` : ""}scale=512:512:force_original_aspect_ratio=decrease,showinfo`,
        // JPEG uses full-range YUV explicitly; FFmpeg9 rejects inherited
        // limited-range video formats at its default MJPEG compliance level.
        "-pix_fmt",
        "yuvj420p",
        "-q:v",
        "3",
        "-fs",
        String(policy.max_frame_bytes),
        "-y",
        output,
      ]);
      const outputStat = await stat(output).catch(
        (error: NodeJS.ErrnoException) => {
          if (error.code === "ENOENT") return undefined;
          throw error;
        },
      );
      if (!outputStat) {
        omitted.push(at);
        continue;
      }
      const size = outputStat.size;
      if (size < 4 || size > policy.max_frame_bytes)
        throw new Error("Video frame exceeds bounds");
      const frame = await readFile(output);
      if (
        frame[0] !== 255 ||
        frame[1] !== 216 ||
        frame[frame.length - 2] !== 255 ||
        frame[frame.length - 1] !== 217
      )
        throw new Error("Incomplete JPEG frame");
      const pts =
        /pts_time:\s*([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)/.exec(
          info.stderr,
        );
      if (!pts) throw new Error("Sample frame timestamp is unavailable");
      const timestamp = (animatedGif ? 0 : at) + Number(pts[1]);
      if (
        !Number.isFinite(timestamp) ||
        timestamp < 0 ||
        timestamp > bounded + 0.1
      )
        throw new Error("Sample frame timestamp exceeds bounds");
      if (timestamp >= bounded) continue;
      if (!frames.some((f) => Math.abs(f.timestamp - timestamp) < 0.00001))
        frames.push({ bytes: frame, timestamp });
    }
    if (!frames.length)
      throw new Error("Video has no frames within the sampling interval");
    const hasAudio = /Stream[^\r\n]*Audio:/.test(metadata.stderr);
    let audio: Uint8Array | undefined;
    let audioProblem: string | undefined;
    if (includeAudio && hasAudio) {
      try {
        const path = join(root, "audio.wav");
        await run("audio", [
          ...inputArgs,
          "-i",
          input,
          "-map",
          "0:a:0",
          "-vn",
          "-t",
          String(bounded),
          "-ac",
          "1",
          "-ar",
          "16000",
          "-c:a",
          "pcm_s16le",
          "-fs",
          String(policy.max_audio_bytes),
          "-y",
          path,
        ]);
        const length = (await stat(path)).size;
        if (length <= 44 || length > policy.max_audio_bytes)
          throw new Error("Video audio exceeds bounds");
        audio = await readFile(path);
      } catch {
        if (signal?.aborted) throw signal.reason;
        audioProblem = "audio_extraction_unavailable";
      }
    }
    return {
      frames,
      audio,
      hasAudio,
      audioProblem,
      quality: {
        ffmpeg_version: version,
        sampling_policy: "uniform-bounded-v1",
        requested_timestamps: requested,
        sampled_timestamps: frames.map((f) => f.timestamp),
        source_duration_seconds: duration,
        sampled_duration_seconds: bounded,
        truncated: duration > bounded,
        frame_coverage: "sampled",
        partial_coverage: true,
        ...(omitted.length ? { omitted_sample_timestamps: omitted } : {}),
        ...(animatedGif
          ? {
              source_media_type: "image/gif",
              animated_timeline: true,
            }
          : {}),
      },
      preprocessingDigest: sampling.preprocessingDigest,
    };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

/** Resolve the actual sampling implementation before reading or transforming source bytes. */
async function videoSampling(
  policy: Policy,
  secretNames: readonly string[],
  signal?: AbortSignal,
  mediaType?: string,
) {
  const executable = policy.ffmpeg_executable;
  if (!executable)
    throw new Error(
      "FFmpeg runtime is unavailable; use nous runtime install ffmpeg or configure an executable",
    );
  const secretSet = new Set(secretNames.map((name) => name.toUpperCase()));
  const env: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(process.env))
    if (!secretSet.has(name.toUpperCase())) env[name] = value;
  const run = (
    phase: "version" | "metadata" | "frame" | "audio",
    args: string[],
  ) =>
    new Promise<{ stdout: string; stderr: string }>((resolve, reject) => {
      execFile(
        executable,
        args,
        {
          windowsHide: true,
          env,
          timeout: policy.process_timeout_ms,
          maxBuffer: 65536,
          signal,
        },
        (error, stdout, stderr) => {
          if (error)
            reject(
              new Error(
                `FFmpeg ${phase} execution unavailable, failed or exceeded bounds`,
              ),
            );
          else resolve({ stdout, stderr });
        },
      );
    });
  const version =
    (await run("version", ["-version"])).stdout.split(/\r?\n/)[0] ?? "";
  if (!version.startsWith("ffmpeg version "))
    throw new Error("Configured executable is not FFmpeg");
  return {
    run,
    version,
    preprocessingDigest: videoSamplingIdentity(policy, mediaType),
  };
}

/** Cache identity describes requested sampling; runtime facts are recorded with produced results. */
export function videoSamplingIdentity(policy: Policy, mediaType?: string) {
  return canonicalDigest({
    videoFrameInputIdentity,
    ...(mediaType === "image/gif"
      ? { source_media_type: mediaType, animated_sampling: "gif-timeline-v1" }
      : {}),
    max_frames: policy.max_frames,
    max_frame_bytes: policy.max_frame_bytes,
    max_audio_bytes: policy.max_audio_bytes,
    max_video_seconds: policy.max_video_seconds,
    frame_end_margin_seconds: policy.frame_end_margin_seconds,
  });
}
