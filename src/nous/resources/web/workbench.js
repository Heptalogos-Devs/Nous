import { t, initializeLocale, message } from "./i18n.js";
import { $, state, el, api, op, notify, error, busy, form } from "./client.js";
import {
  query,
  loadSources,
  initializeKnowledge,
  resetKnowledge,
  renderKnowledge,
} from "./knowledge.js";
import {
  loadJobs,
  loadModels,
  loadPrompt,
  initializeManagement,
  renderManagement,
  resetManagement,
} from "./management.js";
import { initializeAccess, loadCredentials, clearIssuedToken } from "./access.js";
import { initializeBundles, clearBundle } from "./bundles.js";
import { initializeMedia, loadMedia, clearMedia, renderMedia } from "./media.js";
import {
  initializeMaintenance,
  loadMaintenance,
  clearMaintenance,
  renderMaintenance,
} from "./maintenance.js";
await initializeLocale();
const titles = {
  search: "web-search",
  media: "web-media-workspace",
  maintenance: "web-maintenance",
  imports: "web-imports",
  sources: "web-sources",
  contexts: "web-contexts",
  jobs: "web-jobs",
  models: "web-models",
  prompts: "cli-prompt",
  backups: "web-backups",
  bundles: "web-bundles",
  credentials: "web-credentials",
};
async function subjects() {
  const rows = await api(`/api/namespaces/${encodeURIComponent(state.namespace)}/subjects`);
  const select = $("subject");
  select.replaceChildren(
    ...rows.map((row) => {
      const option = el("option", "", row.name);
      option.value = row.id;
      return option;
    }),
  );
  if (rows.some((r) => r.id === state.subject)) select.value = state.subject;
  state.subject = select.value;
  localStorage.setItem("nous.subject", state.subject);
}
async function namespaces() {
  const rows = await api("/api/namespaces");
  const select = $("namespace");
  select.replaceChildren(
    ...rows.map((row) => {
      const option = el("option", "", row.name);
      option.value = row.id;
      return option;
    }),
  );
  if (rows.some((r) => r.id === state.namespace)) select.value = state.namespace;
  state.namespace = select.value;
  localStorage.setItem("nous.namespace", state.namespace);
  if (state.namespace) await subjects();
  else {
    state.subject = "";
    $("subject").replaceChildren();
  }
}
async function navigate(reload = true) {
  const page = location.hash.slice(1) || "search";
  if (!titles[page]) {
    location.hash = "search";
    return;
  }
  document
    .querySelectorAll("[data-panel]")
    .forEach((node) => (node.hidden = node.dataset.panel !== page));
  document
    .querySelectorAll("[data-page]")
    .forEach((node) => node.classList.toggle("active", node.dataset.page === page));
  message($("breadcrumb"), titles[page]);
  if (page !== "credentials") clearIssuedToken();
  if (!state.token) return;
  if (
    !state.subject &&
    ["search", "sources", "contexts", "jobs", "media", "maintenance"].includes(page)
  ) {
    notify("web-select-subject");
    return;
  }
  const load = {
    sources: loadSources,
    media: loadMedia,
    maintenance: loadMaintenance,
    jobs: loadJobs,
    models: loadModels,
    prompts: loadPrompt,
    credentials: loadCredentials,
  }[page];
  if (load && reload) await load();
}
form("connect-form", async () => {
  $("login-error").textContent = "";
  state.token = $("token").value.trim();
  try {
    const value = await api("/api/status");
    if (value.status === "running") message($("instance-status"), "web-service-running");
    else $("instance-status").textContent = value.status;
    await namespaces();
    $("login").hidden = true;
    $("workspace").hidden = false;
    $("token").value = "";
    await navigate();
  } catch (e) {
    $("login-error").textContent = e.message;
    state.token = "";
  }
});
$("logout").onclick = () => {
  document.querySelectorAll("dialog:not(#entity-dialog)").forEach((dialog) => dialog.remove());
  $("login-error").textContent = "";
  state.token = "";
  clearIssuedToken();
  clearBundle();
  clearMedia();
  clearMaintenance();
  resetKnowledge();
  resetManagement();
  for (const id of [
    "credential-list",
    "job-list",
    "call-list",
    "backup-result",
    "restore-result",
    "prompt-history",
    "prompt-output",
  ])
    $(id).replaceChildren();
  $("models-json").value = "";
  $("prompt-template").value = "";
  $("notice").hidden = true;
  $("workspace").hidden = true;
  $("login").hidden = false;
  $("token").value = "";
};
window.addEventListener("nous:disconnect", () => $("logout").click());
$("namespace").onchange = () =>
  busy($("namespace"), async () => {
    state.namespace = $("namespace").value;
    resetKnowledge();
    resetManagement();
    clearBundle();
    clearMedia();
    clearMaintenance();
    localStorage.setItem("nous.namespace", state.namespace);
    await subjects();
    await navigate();
  });
$("subject").onchange = () => {
  state.subject = $("subject").value;
  resetKnowledge();
  resetManagement();
  clearBundle();
  clearMedia();
  clearMaintenance();
  localStorage.setItem("nous.subject", state.subject);
  navigate().catch(error);
};
$("new-entity").onclick = () => $("entity-dialog").showModal();
$("close-dialog").onclick = () => $("entity-dialog").close();
form("entity-form", async () => {
  const kind = $("entity-kind").value;
  const path =
    kind === "namespace"
      ? "/api/namespaces"
      : `/api/namespaces/${encodeURIComponent(state.namespace)}/subjects`;
  if (kind === "subject" && !state.namespace) throw new Error(t("web-select-namespace"));
  const value = await api(path, {
    method: "POST",
    body: { name: $("entity-name").value, operation_id: op() },
  });
  if (kind === "namespace") {
    state.namespace = value.id;
    state.subject = "";
  } else state.subject = value.id;
  await namespaces();
  $("entity-dialog").close();
  $("entity-name").value = "";
  notify("web-entity-created");
  await navigate();
});
initializeKnowledge();
initializeManagement();
initializeAccess();
initializeBundles();
initializeMedia();
initializeMaintenance();
window.addEventListener("nous:locale", () => {
  navigate(false).catch(error);
  renderKnowledge();
  renderManagement();
  renderMedia();
  renderMaintenance();
  if (state.token && location.hash === "#credentials") loadCredentials().catch(error);
});
window.addEventListener("hashchange", () => navigate().catch(error));
navigate().catch(error);
