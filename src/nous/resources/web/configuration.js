import { $, api, button, el, error, form, jsonView, op, state, table, notify } from "./client.js";
import { message, t } from "./i18n.js";

let keys, scope, editor, origin, definition, history;
let catalog = [];
let generation = 0;

function labeled(key, control) {
  const label = el("label");
  label.append(message(el("span"), key), control);
  return label;
}

function params() {
  return new URLSearchParams({ namespace_id: state.namespace, subject_id: state.subject });
}

async function loadKey() {
  const key = keys.value;
  if (!key) return;
  const requested = ++generation;
  const value = await api(`/api/configuration/${encodeURIComponent(key)}?${params()}`);
  if (requested !== generation) return;
  editor.value = JSON.stringify(value.value, null, 2);
  const inherited = value.origin.split(":")[0];
  scope.value = ["instance", "namespace", "subject"].includes(inherited) ? inherited : "subject";
  if (!state.subject && scope.value === "subject") scope.value = "namespace";
  message(origin, "web-models-origin", { origin: value.origin });
  const registered = catalog.find((item) => item.key === key);
  definition.replaceChildren(
    message(
      el("p", "muted"),
      registered.write_access === "instance_manage"
        ? "web-config-instance-access"
        : "web-config-scoped-access",
    ),
    message(el("h3"), "web-config-schema"),
    jsonView(registered.value_schema),
    message(el("h3"), "web-config-default"),
    jsonView(registered.default_value),
  );
  history.replaceChildren();
}

export async function loadConfiguration() {
  const value = await api(`/api/configuration?${params()}`);
  catalog = value.items;
  const selected = keys.value;
  keys.replaceChildren(
    ...catalog.map((item) => {
      const option = el("option", "", item.key);
      option.value = item.key;
      return option;
    }),
  );
  if (catalog.some((item) => item.key === selected)) keys.value = selected;
  await loadKey();
}

async function save(value) {
  await api(`/api/configuration/${encodeURIComponent(keys.value)}?${params()}`, {
    method: "PUT",
    body: { operation_id: op(), scope: scope.value, value },
  });
  notify("web-config-saved");
  await loadKey();
}

async function loadHistory() {
  const key = keys.value,
    requested = generation;
  const value = await api(
    `/api/configuration/${encodeURIComponent(key)}/history?${params()}&limit=20`,
  );
  if (requested !== generation || key !== keys.value) return;
  history.replaceChildren(
    table(
      [t("web-models-save-scope"), t("cli-commit-seq"), t("web-actions")],
      value.items.map((item) => [
        item.scope,
        item.commit_seq,
        button(t("web-config-use-value"), () => {
          scope.value = item.scope.split(":")[0];
          editor.value = JSON.stringify(item.value, null, 2);
        }),
      ]),
    ),
  );
}

export function initializeConfiguration() {
  keys = el("select");
  scope = el("select");
  for (const name of ["instance", "namespace", "subject"]) {
    const option = message(el("option"), `web-scope-${name}`);
    option.value = name;
    scope.append(option);
  }
  editor = el("textarea");
  editor.rows = 18;
  editor.spellcheck = false;
  origin = el("p", "muted");
  definition = el("div");
  history = el("div");
  const heading = el("div", "page-heading");
  heading.append(
    message(el("h1"), "web-configuration"),
    message(button("", loadConfiguration), "web-refresh"),
  );
  const input = el("form", "form-sheet");
  input.id = "configuration-form";
  const actions = el("div", "button-row");
  const submit = message(el("button"), "web-config-save");
  submit.type = "submit";
  actions.append(
    submit,
    message(
      button("", () => save(null)),
      "web-config-reset",
    ),
  );
  input.append(
    labeled("web-models-save-scope", scope),
    labeled("web-config-value", editor),
    actions,
  );
  const details = el("details");
  details.append(message(el("summary"), "web-config-definition"), definition);
  $("configuration-panel").append(
    heading,
    labeled("web-config-key", keys),
    origin,
    input,
    details,
    message(button("", loadHistory), "web-config-history"),
    history,
  );
  keys.addEventListener("change", () => loadKey().catch(error));
  form("configuration-form", () => save(JSON.parse(editor.value)));
}

export function resetConfiguration() {
  generation += 1;
  catalog = [];
  keys.replaceChildren();
  editor.value = "";
  origin.textContent = "";
  definition.replaceChildren();
  history.replaceChildren();
}
