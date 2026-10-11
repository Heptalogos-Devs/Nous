// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { UserContent } from "ai";
import { z } from "zod";
import type { ModelProfile } from "./profiles.js";

export type DirectMedia = { bytes: Uint8Array; mediaType: string };
export type InputModalities = {
  visual: boolean;
  audio: boolean;
  text: boolean;
};
export const directAudioFormats: Readonly<Record<string, string>> = {
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/aac": "aac",
  "audio/mp4": "m4a",
};
export class InputUnavailable extends Error {}
const access = z.enum(["original", "representation", "unavailable"]);
export const channelAccessSchema = z.strictObject({
  visual: access,
  audio: access,
  source_text: access,
});
export type ChannelAccess = z.infer<typeof channelAccessSchema>;

export function originalMediaAccess(input: InputModalities): ChannelAccess {
  return {
    visual: input.visual ? "original" : "unavailable",
    audio: input.audio ? "original" : "unavailable",
    source_text: "unavailable",
  };
}

/** Admission uses the actual physical input and the frozen candidate route. */
export function generationInput(
  profile: ModelProfile,
  content: UserContent,
  media?: DirectMedia,
): InputModalities {
  if (!["openai-chat", "openai-responses"].includes(profile.protocol))
    throw new InputUnavailable("generation_protocol_unavailable");
  const files =
    typeof content === "string"
      ? []
      : content.filter((part) => part.type === "file");
  const visual =
    Boolean(media?.mediaType.startsWith("video/")) ||
    files.some((file) => file.mediaType.startsWith("image/")) ||
    (typeof content !== "string" &&
      content.some((part) => part.type === "image"));
  if (visual && !media && !profile.capabilities.includes("image_input"))
    throw new InputUnavailable("image_input_unavailable");
  if (media) {
    if (profile.protocol !== "openai-chat")
      throw new InputUnavailable("direct_media_protocol_unavailable");
    const required = media.mediaType.startsWith("audio/")
      ? "audio_input"
      : "video_input";
    if (!profile.capabilities.includes(required))
      throw new InputUnavailable(`${required}_unavailable`);
    if (
      (required === "audio_input" && !directAudioFormats[media.mediaType]) ||
      (required === "video_input" && !media.mediaType.startsWith("video/"))
    )
      throw new InputUnavailable("direct_media_format_unavailable");
    if (
      !media.bytes.length ||
      media.bytes.length > (required === "audio_input" ? 25165824 : 67108864)
    )
      throw new InputUnavailable("direct_media_bounds_exceeded");
  }
  return {
    visual,
    audio: Boolean(
      media &&
      (media.mediaType.startsWith("audio/") ||
        (media.mediaType.startsWith("video/") &&
          profile.capabilities.includes("audio_input"))),
    ),
    text:
      typeof content === "string" ||
      content.some((part) => part.type === "text"),
  };
}

export function representedAccess(input: ChannelAccess): ChannelAccess {
  return Object.fromEntries(
    Object.entries(input).map(([channel, mode]) => [
      channel,
      mode === "unavailable" ? mode : "representation",
    ]),
  ) as ChannelAccess;
}
