import { t, message } from "./i18n.js";
import { $, state, el, api, op, button, jsonView, subjectPath, form, download } from "./client.js";

let selectionGeneration = 0;
let selected = null,
  lineage = null,
  source = null,
  previewURL = null;
function caption(key) {
  return message(el("span"), key);
}
function field(id, key, type = "text") {
  const label = el("label");
  const input = el("input");
  input.id = id;
  input.type = type;
  if (type === "number") {
    input.min = "0";
    input.step = "1";
  }
  label.append(caption(key), input);
  return label;
}
function selector(id, key, entries) {
  const label = el("label"),
    input = el("select");
  input.id = id;
  for (const [value, title] of entries) {
    const item = message(el("option"), title);
    item.value = value;
    input.append(item);
  }
  label.append(caption(key), input);
  return label;
}
function value(id) {
  return $(id).value.trim();
}
function integer(id) {
  const raw = value(id),
    result = Number(raw);
  if (!raw || !Number.isSafeInteger(result) || result < 0)
    throw new Error(t("web-media-coordinate-invalid"));
  return result;
}
function coordinates(kind, prefix = "media") {
  if (kind === "pixels")
    return {
      pixel_rect: {
        x: integer(`${prefix}-x`),
        y: integer(`${prefix}-y`),
        width: integer(`${prefix}-width`),
        height: integer(`${prefix}-height`),
      },
    };
  if (kind === "time")
    return {
      time_span: { start_ms: integer(`${prefix}-start`), end_ms: integer(`${prefix}-end`) },
    };
  if (kind === "pages")
    return { page_start: integer(`${prefix}-start`), page_end: integer(`${prefix}-end`) };
  if (kind === "json") return { json_pointer: $(`${prefix}-pointer`).value };
  return { text_start: integer(`${prefix}-start`), text_end: integer(`${prefix}-end`) };
}
function contentPath(reference) {
  const id = encodeURIComponent(reference.id);
  if (reference.kind === "material") return subjectPath(`/materials/${id}/content`);
  if (reference.kind === "source_region") return subjectPath(`/regions/${id}/content`);
  return subjectPath(`/media/${reference.kind}/${id}/content`);
}
async function selectMedia(reference) {
  const generation = ++selectionGeneration;
  const nextSource = await api("/api/sources/read", {
    method: "POST",
    body: { subject_id: state.subject, reference },
  });
  const nextLineage = await api(subjectPath("/media/lineage"), { method: "POST", body: reference });
  if (generation !== selectionGeneration) return;
  selected = reference;
  source = nextSource;
  lineage = nextLineage;
  $("media-locator").value = `${reference.kind.replaceAll("_", "-")}:${reference.id}`;
  renderMedia();
}
export async function loadMedia() {
  if (value("media-locator")) {
    const resolution = await api("/api/references", {
      method: "POST",
      body: {
        subject_id: state.subject,
        reference: value("media-locator"),
      },
    });
    if (resolution.reference.kind !== "media") throw new Error(t("web-media-required"));
    await selectMedia(resolution.reference.reference);
  }
}
export function clearMedia() {
  selectionGeneration++;
  selected = lineage = source = null;
  if (previewURL) URL.revokeObjectURL(previewURL);
  previewURL = null;
  $("media-locator").value = "";
  $("media-detail").replaceChildren();
  $("media-results").replaceChildren();
}
async function preview() {
  const data = await api(contentPath(selected), { responseType: "blob" });
  if (previewURL) URL.revokeObjectURL(previewURL);
  previewURL = URL.createObjectURL(data);
  const output = $("media-preview");
  output.replaceChildren();
  let view;
  if (data.type.startsWith("image/")) {
    view = el("img");
    view.alt = t("web-media-preview");
  } else if (/^(audio|video)\//.test(data.type)) {
    view = el(data.type.startsWith("audio/") ? "audio" : "video");
    view.controls = true;
  } else {
    output.textContent = await data.text();
    return;
  }
  view.src = previewURL;
  output.append(view);
}
export function renderMedia() {
  const panel = $("media-detail");
  panel.replaceChildren();
  if (!selected) return;
  panel.append(el("h2", "ref", $("media-locator").value));
  const actions = el("div", "button-row");
  actions.append(
    button(t("web-media-preview"), preview),
    button(t("web-media-download"), () =>
      download(
        contentPath(selected),
        source.material?.filename || `${selected.kind}-${selected.id}`,
      ),
    ),
  );
  panel.append(
    actions,
    Object.assign(el("div", "body-text media-preview"), { id: "media-preview" }),
  );
  const detail = el("details");
  detail.append(message(el("summary"), "web-full-record"), jsonView(source));
  panel.append(detail);
  if (selected.kind === "representation" || selected.kind === "derived_region") {
    const ref =
      selected.kind === "representation"
        ? lineage.representations.find(
            (r) => `${r.reference.id}@${r.reference.revision}` === selected.id,
          )?.reference
        : lineage.derived_regions.find((r) => r.id === selected.id)?.representation;
    if (ref)
      panel.append(
        button(t("web-media-history"), async () => {
          const history = await api(
            subjectPath(`/media/representations/${encodeURIComponent(ref.id)}/history`),
          );
          const historyPanel = el("div");
          for (const row of history)
            historyPanel.append(
              button(`@${row.reference.revision} · ${row.kind}`, () =>
                selectMedia({
                  kind: "representation",
                  id: `${row.reference.id}@${row.reference.revision}`,
                }),
              ),
            );
          panel.append(historyPanel);
        }),
      );
  }
  panel.append(message(el("h3"), "web-media-lineage"));
  for (const row of lineage.representations)
    panel.append(
      button(row.addresses?.exact || `${row.kind} @${row.reference.revision}`, () =>
        selectMedia({
          kind: "representation",
          id: `${row.reference.id}@${row.reference.revision}`,
        }),
      ),
    );
  for (const row of lineage.derived_regions)
    panel.append(
      button(row.addresses?.exact || row.id, () =>
        selectMedia({ kind: "derived_region", id: row.id }),
      ),
    );
  if (lineage.unavailable_inputs.length)
    panel.append(
      el(
        "p",
        "hint",
        t("web-media-unavailable-inputs", { count: lineage.unavailable_inputs.length }),
      ),
    );
}
export function initializeMedia() {
  const host = $("media-controls");
  const open = el("form", "form-sheet");
  open.id = "media-open";
  open.append(field("media-locator", "web-media-locator"), message(el("button"), "web-media-open"));
  const selection = el("form", "form-sheet");
  selection.id = "media-select";
  selection.append(
    message(el("h2"), "web-media-select"),
    selector("media-selector", "web-media-coordinate-kind", [
      ["pixels", "web-media-pixels"],
      ["time", "web-media-time"],
      ["pages", "web-media-pages"],
      ["text", "web-media-text"],
      ["json", "web-media-json"],
    ]),
  );
  const grid = el("div", "media-coordinate-grid");
  for (const [id, key] of [
    ["x", "web-media-x"],
    ["y", "web-media-y"],
    ["width", "web-media-width"],
    ["height", "web-media-height"],
    ["start", "web-media-start"],
    ["end", "web-media-end"],
  ])
    grid.append(field(`media-${id}`, key, "number"));
  selection.append(
    grid,
    field("media-pointer", "web-media-pointer"),
    message(el("button"), "web-media-select"),
  );
  const derive = el("form", "form-sheet");
  derive.id = "media-derive";
  derive.append(
    message(el("h2"), "web-media-derive"),
    selector("media-action", "web-media-transformation", [
      ["crop", "web-media-crop"],
      ["clip", "web-media-clip"],
      ["audio_track", "web-media-audio-track"],
      ["sample_frames", "web-media-sample-frames"],
    ]),
  );
  const deriveGrid = el("div", "media-coordinate-grid");
  for (const [id, key] of [
    ["x", "web-media-x"],
    ["y", "web-media-y"],
    ["width", "web-media-width"],
    ["height", "web-media-height"],
    ["start", "web-media-start-ms"],
    ["end", "web-media-end-ms"],
  ])
    deriveGrid.append(field(`derive-${id}`, key, "number"));
  derive.append(
    deriveGrid,
    field("derive-samples", "web-media-samples"),
    message(el("button"), "web-media-derive"),
  );
  const upload = el("form", "form-sheet");
  upload.id = "media-upload";
  upload.append(
    field("media-file", "web-media-upload", "file"),
    message(el("button"), "web-media-upload"),
  );
  host.append(open, upload, selection, derive);
  $("media-file").required = true;
  function showFields(prefix, fields) {
    for (const name of ["x", "y", "width", "height", "start", "end", "pointer", "samples"]) {
      const node = $(`${prefix}-${name}`);
      if (node) node.closest("label").hidden = !fields.includes(name);
    }
  }
  function selectionFields() {
    const kind = value("media-selector");
    showFields(
      "media",
      kind === "pixels"
        ? ["x", "y", "width", "height"]
        : kind === "json"
          ? ["pointer"]
          : ["start", "end"],
    );
    if (["time", "text", "pages"].includes(kind)) {
      const suffix = { time: "ms", text: "text", pages: "page" }[kind];
      for (const name of ["start", "end"])
        message(
          $(`media-${name}`).closest("label").querySelector("span"),
          `web-media-${name}-${suffix}`,
        );
    }
  }
  function deriveFields() {
    const action = value("media-action");
    showFields(
      "derive",
      action === "crop"
        ? ["x", "y", "width", "height"]
        : action === "clip"
          ? ["start", "end"]
          : action === "sample_frames"
            ? ["samples"]
            : [],
    );
  }
  $("media-selector").onchange = selectionFields;
  selectionFields();
  $("media-action").onchange = deriveFields;
  deriveFields();
  form("media-upload", async () => {
    const file = $("media-file").files[0];
    if (!file) return;
    const params = new URLSearchParams({
      namespace_id: state.namespace,
      owner_subject_id: state.subject,
      source: file.name,
      operation_id: op(),
    });
    const result = await api(`/api/media/materials?${params}`, {
      method: "POST",
      headers: { "Content-Type": file.type || "application/octet-stream" },
      body: file,
    });
    await selectMedia({ kind: "material", id: result.result.id });
    $("media-file").value = "";
  });
  form("media-open", loadMedia);
  form("media-select", async () => {
    if (!selected) throw new Error(t("web-media-required"));
    const kind = value("media-selector"),
      coords = coordinates(kind);
    if (["representation", "derived_region"].includes(selected.kind)) {
      const representation =
        selected.kind === "representation"
          ? lineage.representations.find(
              (r) => `${r.reference.id}@${r.reference.revision}` === selected.id,
            ).reference
          : lineage.derived_regions.find((r) => r.id === selected.id).representation;
      const selector =
        kind === "text"
          ? { kind: "text_span", start: coords.text_start, end: coords.text_end }
          : kind === "json"
            ? { kind: "json_pointer", ...coords }
            : { kind: "media", ...coords };
      const region = await api(subjectPath("/media/derived-regions"), {
        method: "POST",
        body: {
          operation_id: op(),
          representation,
          selector,
          parent_id: selected.kind === "derived_region" ? selected.id : null,
        },
      });
      await selectMedia({ kind: "derived_region", id: region.id });
    } else {
      if (selected.kind !== "material") throw new Error(t("web-media-original-required"));
      const region = await api(subjectPath("/media/source-regions"), {
        method: "POST",
        body: {
          operation_id: op(),
          region: { id: op(), material_id: selected.id, ...coords },
        },
      });
      await selectMedia({ kind: "source_region", id: region.id });
    }
  });
  form("media-derive", async () => {
    if (!selected) throw new Error(t("web-media-required"));
    const action = value("media-action"),
      transformation = { action };
    if (action === "crop") Object.assign(transformation, coordinates("pixels", "derive"));
    if (action === "clip") Object.assign(transformation, coordinates("time", "derive"));
    if (action === "sample_frames") {
      transformation.sample_at_ms = value("derive-samples")
        .split(/[,\s]+/)
        .map(Number);
      if (
        !value("derive-samples") ||
        transformation.sample_at_ms.some((p) => !Number.isSafeInteger(p) || p < 0)
      )
        throw new Error(t("web-media-coordinate-invalid"));
    }
    const rows = await api(subjectPath("/media/derive"), {
      method: "POST",
      body: { operation_id: op(), source: selected, transformation },
    });
    $("media-results").replaceChildren(
      ...rows.map((row) =>
        button(row.addresses?.exact || row.reference.id, () =>
          selectMedia({
            kind: "representation",
            id: `${row.reference.id}@${row.reference.revision}`,
          }),
        ),
      ),
    );
    if (rows.length)
      await selectMedia({
        kind: "representation",
        id: `${rows[0].reference.id}@${rows[0].reference.revision}`,
      });
  });
}
