import { $, api, button, el, jsonView, state } from "./client.js";
import { message, t } from "./i18n.js";

let activeDialog;

function labeled(key, control) {
  const label = el("label");
  label.append(message(el("span"), key), control);
  return label;
}

function editor(value, rows) {
  const node = el("textarea");
  node.value = JSON.stringify(value, null, 2);
  node.rows = rows;
  node.spellcheck = false;
  return node;
}

function section(key, value, open = false) {
  const node = el("details");
  node.open = open;
  node.append(message(el("summary"), key), jsonView(value));
  return node;
}

export function closeModelContract() {
  activeDialog?.close();
}

export async function showModelContract(role, editPrompt) {
  closeModelContract();
  const namespace = state.namespace;
  const subject = state.subject;
  const params = new URLSearchParams({ namespace_id: namespace, subject_id: subject });
  async function request(path, body) {
    if (state.namespace !== namespace || state.subject !== subject)
      throw new Error(t("web-connection-changed"));
    return api(`${path}?${params}`, { method: "POST", body });
  }
  let contract = await request("/api/models/contract", { role });
  const dialog = el("dialog", "model-contract");
  activeDialog = dialog;
  dialog.addEventListener("close", () => {
    dialog.remove();
    if (activeDialog === dialog) activeDialog = undefined;
  });
  const heading = el("div", "page-heading");
  heading.append(
    message(el("h2"), "web-model-contract-title", { role }),
    message(
      button("", () => dialog.close()),
      "web-dismiss",
    ),
  );
  const variants = el("select");
  for (const value of contract.variants) {
    const option = el("option", "", value);
    option.value = value;
    variants.append(option);
  }
  variants.value = contract.variant;
  const schemas = el("div");
  function renderContract() {
    schemas.replaceChildren(
      message(
        el("p", "muted"),
        contract.output_mode === "text" ? "web-output-mode-text" : "web-output-mode-json",
      ),
      section("web-model-input-schema", contract.input_schema, true),
      section("web-model-output-schema", contract.output_schema, true),
      section("web-model-provider-formats", contract.provider_response_formats),
      section("web-model-active-prompt", contract.prompt),
    );
  }
  renderContract();
  const input = editor({}, 8);
  const media = editor([], 3);
  const sources = editor([], 3);
  const invocation = el("div");
  const preview = message(
    button("", async () => {
      const value = await request("/api/models/preview", {
        role,
        variant: variants.value,
        input: JSON.parse(input.value),
        media: JSON.parse(media.value),
        source_ids: JSON.parse(sources.value),
      });
      invocation.replaceChildren(
        message(el("h3"), "web-model-rendered-input"),
        el("pre", "json-view", value.invocation.prompt.rendered),
        section("web-model-invocation", value.invocation, true),
      );
    }),
    "web-preview-invocation",
  );
  variants.addEventListener("change", async () => {
    preview.disabled = true;
    try {
      contract = await request("/api/models/contract", { role, variant: variants.value });
      renderContract();
      invocation.replaceChildren();
    } catch (error) {
      invocation.replaceChildren(el("p", "error", error.message));
    } finally {
      preview.disabled = false;
    }
  });
  const actions = el("div", "button-row");
  actions.append(
    preview,
    message(
      button("", async () => {
        dialog.close();
        await editPrompt(contract.prompt_id);
      }),
      "web-edit-role-prompt",
    ),
  );
  const supplemental = el("details");
  supplemental.append(
    message(el("summary"), "web-model-media-sources"),
    labeled("web-model-media-refs", media),
    labeled("web-model-source-ids", sources),
  );
  dialog.append(
    heading,
    labeled("web-model-contract-variant", variants),
    schemas,
    labeled("web-model-input-values", input),
    supplemental,
    actions,
    invocation,
  );
  document.body.append(dialog);
  dialog.showModal();
}
