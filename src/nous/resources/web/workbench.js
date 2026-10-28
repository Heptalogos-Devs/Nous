import { $, state, el, api, op, notice, error, busy, form } from "./client.js";
import { query, loadSources, initializeKnowledge } from "./knowledge.js";
import { loadJobs, loadModels, loadPrompt, initializeManagement } from "./management.js";
const titles = {
  search: "认知库",
  imports: "接纳资料",
  sources: "来源与恢复",
  contexts: "工作上下文",
  jobs: "持久任务",
  models: "模型与角色",
  prompts: "Prompt",
  backups: "实例备份",
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
async function navigate() {
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
  $("breadcrumb").textContent = titles[page];
  if (!state.token) return;
  if (!state.subject && ["search", "sources", "contexts", "jobs"].includes(page)) {
    notice("请先创建或选择主体");
    return;
  }
  const load = { sources: loadSources, jobs: loadJobs, models: loadModels, prompts: loadPrompt }[
    page
  ];
  if (load) await load();
}
form("connect-form", async () => {
  state.token = $("token").value.trim();
  try {
    const value = await api("/api/status");
    $("instance-status").textContent = `提交位置 ${value.commit_seq}`;
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
  state.token = "";
  $("workspace").hidden = true;
  $("login").hidden = false;
  $("token").value = "";
};
$("namespace").onchange = () =>
  busy($("namespace"), async () => {
    state.namespace = $("namespace").value;
    localStorage.setItem("nous.namespace", state.namespace);
    await subjects();
    await navigate();
  });
$("subject").onchange = () => {
  state.subject = $("subject").value;
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
  if (kind === "subject" && !state.namespace) throw new Error("请先创建命名空间");
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
  notice("已创建");
  await navigate();
});
initializeKnowledge();
initializeManagement();
window.addEventListener("hashchange", () => navigate().catch(error));
navigate().catch(error);
