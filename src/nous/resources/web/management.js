import { t, locale, message } from "./i18n.js";
import {
  $,
  state,
  el,
  api,
  subjectPath,
  op,
  table,
  button,
  form,
  notify,
  busy,
  jsonView,
  chip,
  download,
} from "./client.js";
let lastJobs, lastCalls, lastPrompt;
export async function loadJobs() {
  const params = new URLSearchParams({ namespace_id: state.namespace, subject_id: state.subject });
  const jobs = await api(`/api/jobs?${params}`);
  lastJobs = jobs;
  renderJobs(jobs);
}
function renderJobs(jobs) {
  const params = new URLSearchParams({ namespace_id: state.namespace, subject_id: state.subject });
  $("job-list").replaceChildren(
    jobs.length
      ? table(
          [t("web-job"), t("web-workflow"), t("web-status"), t("web-actions")],
          jobs.map((job) => {
            const actions = el("div", "button-row");
            for (const action of ["cancel", "resume"])
              actions.append(
                button(action === "cancel" ? t("web-cancel") : t("web-resume"), async () => {
                  await api(`/api/jobs/${job.workflow_id}/${action}?${params}`, { method: "POST" });
                  await loadJobs();
                }),
              );
            return [job.workflow_id, job.workflow_name || job.name, chip(job.status), actions];
          }),
        )
      : el("div", "empty", t("web-no-jobs")),
  );
}
function field(label, value, onchange, type = "text") {
  const l = el("label", "", label);
  const input = el("input");
  input.type = type;
  input.value = value ?? "";
  input.addEventListener("change", () => {
    onchange(type === "number" ? Number(input.value) : input.value);
    $("models-json").value = JSON.stringify(state.config, null, 2);
  });
  l.append(input);
  return l;
}
export async function loadModels() {
  const value = await api(
    `/api/configuration/models?namespace_id=${encodeURIComponent(state.namespace)}&subject_id=${encodeURIComponent(state.subject)}`,
  );
  state.config = value.value;
  $("models-json").value = JSON.stringify(state.config, null, 2);
  renderModels();
  lastCalls = await api(
    `/api/model-calls?namespace_id=${encodeURIComponent(state.namespace)}&subject_id=${encodeURIComponent(state.subject)}`,
  );
  renderCalls(lastCalls);
}
function renderModels() {
  const providers = $("providers");
  providers.replaceChildren();
  for (const [id, p] of Object.entries(state.config.providers)) {
    const card = el("div", "section-card");
    card.append(el("h2", "", t("web-provider-title", { id })));
    const fields = el("div", "form-grid");
    fields.append(
      field("Base URL", p.base_url, (v) => (p.base_url = v)),
      field(t("web-credential-env"), p.credential_env, (v) => (p.credential_env = v)),
      field(t("web-credential-file"), p.credential_file, (v) => (p.credential_file = v || null)),
    );
    card.append(fields);
    providers.append(card);
  }
  const roles = $("roles");
  roles.replaceChildren();
  for (const [id, role] of Object.entries(state.config.roles)) {
    const card = el("div", "section-card");
    card.append(el("h2", "", t("web-role-title", { id })));
    const fields = el("div", "form-grid");
    const route = state.config.routes[role.routes[0]];
    const model = state.config.models[route.model];
    fields.append(
      field(t("web-model-id"), model.name, (v) => (model.name = v)),
      field(
        t("web-daily-calls"),
        role.max_calls_per_day,
        (v) => (role.max_calls_per_day = v),
        "number",
      ),
      field(
        t("web-max-output-tokens"),
        route.max_output_tokens,
        (v) => (route.max_output_tokens = v),
        "number",
      ),
    );
    card.append(fields);
    roles.append(card);
  }
}
function renderCalls(calls) {
  $("call-list").replaceChildren(
    calls.length
      ? table(
          [t("web-call"), t("web-purpose"), t("web-outcome"), "Token", t("web-details")],
          calls.map((c) => [
            c.id,
            c.purpose,
            chip(c.status, c.status !== "succeeded"),
            `${c.usage?.input_tokens ?? "—"} / ${c.usage?.output_tokens ?? "—"}`,
            button(t("web-inspect-call"), () => {
              const d = el("dialog");
              const close = button(t("web-dismiss"), () => {
                d.close();
                d.remove();
              });
              d.append(close, jsonView(c));
              document.body.append(d);
              d.showModal();
            }),
          ]),
        )
      : el("div", "empty", t("web-no-model-calls")),
  );
}
async function saveModels() {
  state.config = JSON.parse($("models-json").value);
  await api(
    `/api/configuration/models?namespace_id=${encodeURIComponent(state.namespace)}&subject_id=${encodeURIComponent(state.subject)}`,
    { method: "PUT", body: { operation_id: op(), scope: "instance", value: state.config } },
  );
  notify("web-models-saved");
  await loadModels();
}
export async function loadPrompt() {
  const id = encodeURIComponent($("prompt-id").value);
  const params = new URLSearchParams({ namespace_id: state.namespace || "instance" });
  const [value, history, catalog] = await Promise.all([
    api(`/api/prompts/${id}?${params}`),
    api(`/api/prompts/${id}/history?${params}`),
    api(`/api/prompts?${params}`),
  ]);
  lastPrompt = { value, history, catalog };
  renderPrompt(value, history, catalog);
}
function renderPrompt(value, history, catalog) {
  const id = encodeURIComponent($("prompt-id").value);
  const params = new URLSearchParams({ namespace_id: state.namespace || "instance" });
  $("prompt-catalog").replaceChildren(
    ...catalog.map((item) => {
      const option = el("option");
      option.value = item.id;
      return option;
    }),
  );
  $("prompt-template").value = value.template;
  message($("prompt-revision"), "web-prompt-revision", {
    revision: value.revision,
    source: value.source === "bundled" ? t("web-bundled-template") : t("web-instance-template"),
  });
  message($("prompt-variables"), "web-prompt-variables", {
    variables: new Intl.ListFormat(locale).format(value.variable_names) || t("web-none"),
  });
  $("prompt-history").replaceChildren(
    table(
      [t("web-revision"), t("web-source"), t("web-status"), t("web-actions")],
      history.map((item) => [
        item.revision,
        item.source === "bundled" ? t("web-bundled-template") : t("web-instance-template"),
        chip(item.active ? t("web-current") : t("web-historical"), !item.active),
        button(t("web-read-revision"), () => {
          $("prompt-template").value = item.template;
          message($("prompt-revision"), "web-editing-revision", { revision: item.revision });
        }),
      ]),
    ),
  );
  for (const [index, item] of history.entries()) {
    if (item.active) continue;
    const cell = $("prompt-history").querySelectorAll("tbody tr")[index].lastElementChild;
    cell.append(
      button(t("web-activate-revision"), async () => {
        await api(`/api/prompts/${id}/activate?${params}`, {
          method: "POST",
          body: { revision: item.revision, operation_id: op() },
        });
        notify("web-revision-activated", { revision: item.revision });
        await loadPrompt();
      }),
    );
  }
}
async function previewPrompt() {
  const variables = JSON.parse($("prompt-input").value);
  if (!variables || Array.isArray(variables) || typeof variables !== "object")
    throw new Error(t("web-variables-object"));
  const value = await api(
    `/api/prompts/${encodeURIComponent($("prompt-id").value)}/preview?namespace_id=${encodeURIComponent(state.namespace || "instance")}`,
    {
      method: "POST",
      body: { variables, template: $("prompt-template").value },
    },
  );
  $("prompt-output").textContent = value.rendered;
}
async function savePrompt() {
  await api(
    `/api/prompts/${encodeURIComponent($("prompt-id").value)}?namespace_id=${encodeURIComponent(state.namespace)}`,
    { method: "PUT", body: { operation_id: op(), template: $("prompt-template").value } },
  );
  notify("web-prompt-saved");
  await loadPrompt();
}
async function createBackup() {
  const value = await api("/api/backups", { method: "POST" });
  const panel = $("backup-result");
  panel.replaceChildren(
    message(el("p"), "web-backup-created", {
      objects: value.object_count,
      commit: value.commit_seq,
    }),
    button(t("web-download-backup"), () =>
      download(`/api/backups/${encodeURIComponent(value.filename)}`, value.filename),
    ),
  );
}
export function initializeManagement() {
  $("refresh-jobs").onclick = () => busy($("refresh-jobs"), loadJobs);
  $("refresh-models").onclick = () => busy($("refresh-models"), loadModels);
  form("models-form", saveModels);
  $("load-prompt").onclick = () => busy($("load-prompt"), loadPrompt);
  form("prompt-form", savePrompt);
  form("prompt-preview-form", previewPrompt);
  $("create-backup").onclick = () => busy($("create-backup"), createBackup);
  setInterval(() => {
    if (state.token && location.hash === "#jobs") loadJobs().catch(() => {});
  }, 5000);
}

export function renderManagement() {
  const page = location.hash;
  if (page === "#jobs" && lastJobs) renderJobs(lastJobs);
  if (page === "#models" && state.config) {
    renderModels();
    if (lastCalls) renderCalls(lastCalls);
  }
  if (page === "#prompts" && lastPrompt) {
    const draft = $("prompt-template").value;
    renderPrompt(lastPrompt.value, lastPrompt.history, lastPrompt.catalog);
    $("prompt-template").value = draft;
  }
}

export function resetManagement() {
  lastJobs = lastCalls = lastPrompt = undefined;
  state.config = null;
  for (const id of [
    "job-list",
    "call-list",
    "providers",
    "roles",
    "prompt-history",
    "prompt-output",
  ])
    $(id).replaceChildren();
  for (const id of ["models-json", "prompt-template", "prompt-input"]) $(id).value = "";
}
