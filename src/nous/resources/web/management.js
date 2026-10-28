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
  notice,
  busy,
  jsonView,
  chip,
  download,
} from "./client.js";
export async function loadJobs() {
  const params = new URLSearchParams({ namespace_id: state.namespace, subject_id: state.subject });
  const jobs = await api(`/api/jobs?${params}`);
  $("job-list").replaceChildren(
    jobs.length
      ? table(
          ["任务", "工作流", "状态", "操作"],
          jobs.map((job) => {
            const actions = el("div", "button-row");
            for (const action of ["cancel", "resume"])
              actions.append(
                button(action === "cancel" ? "取消" : "继续", async () => {
                  await api(`/api/jobs/${job.workflow_id}/${action}?${params}`, { method: "POST" });
                  await loadJobs();
                }),
              );
            return [job.workflow_id, job.workflow_name || job.name, chip(job.status), actions];
          }),
        )
      : el("div", "empty", "暂无持久任务"),
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
  const providers = $("providers");
  providers.replaceChildren();
  for (const [id, p] of Object.entries(state.config.providers)) {
    const card = el("div", "section-card");
    card.append(el("h2", "", `连接 · ${id}`));
    const fields = el("div", "form-grid");
    fields.append(
      field("Base URL", p.base_url, (v) => (p.base_url = v)),
      field("密钥环境变量", p.credential_env, (v) => (p.credential_env = v)),
      field("密钥文件引用", p.credential_file, (v) => (p.credential_file = v || null)),
    );
    card.append(fields);
    providers.append(card);
  }
  const roles = $("roles");
  roles.replaceChildren();
  for (const [id, role] of Object.entries(state.config.roles)) {
    const card = el("div", "section-card");
    card.append(el("h2", "", `角色 · ${id}`));
    const fields = el("div", "form-grid");
    const route = state.config.routes[role.routes[0]];
    const model = state.config.models[route.model];
    fields.append(
      field("模型 ID", model.name, (v) => (model.name = v)),
      field("每日调用额度", role.max_calls_per_day, (v) => (role.max_calls_per_day = v), "number"),
      field(
        "最大输出 Token",
        route.max_output_tokens,
        (v) => (route.max_output_tokens = v),
        "number",
      ),
    );
    card.append(fields);
    roles.append(card);
  }
  const calls = await api(
    `/api/model-calls?namespace_id=${encodeURIComponent(state.namespace)}&subject_id=${encodeURIComponent(state.subject)}`,
  );
  $("call-list").replaceChildren(
    calls.length
      ? table(
          ["调用", "用途", "结果", "Token", "详情"],
          calls.map((c) => [
            c.id,
            c.purpose,
            chip(c.status, c.status !== "succeeded"),
            `${c.usage?.input_tokens ?? "—"} / ${c.usage?.output_tokens ?? "—"}`,
            button("查看实际输入与返回", () => {
              const d = el("dialog");
              const close = button("关闭", () => {
                d.close();
                d.remove();
              });
              d.append(close, jsonView(c));
              document.body.append(d);
              d.showModal();
            }),
          ]),
        )
      : el("div", "empty", "当前主体尚无模型调用"),
  );
}
async function saveModels() {
  state.config = JSON.parse($("models-json").value);
  await api(
    `/api/configuration/models?namespace_id=${encodeURIComponent(state.namespace)}&subject_id=${encodeURIComponent(state.subject)}`,
    { method: "PUT", body: { operation_id: op(), scope: "instance", value: state.config } },
  );
  notice("模型资源已保存，后续调用使用新配置");
  await loadModels();
}
export async function loadPrompt() {
  const id = encodeURIComponent($("prompt-id").value);
  const value = await api(`/api/prompts/${id}?namespace_id=${encodeURIComponent(state.namespace)}`);
  $("prompt-template").value = value.template;
  $("prompt-revision").textContent = `当前修订 ${value.revision}`;
}
async function savePrompt() {
  await api(
    `/api/prompts/${encodeURIComponent($("prompt-id").value)}?namespace_id=${encodeURIComponent(state.namespace)}`,
    { method: "PUT", body: { operation_id: op(), template: $("prompt-template").value } },
  );
  notice("新修订已保存并激活");
  await loadPrompt();
}
async function createBackup() {
  const value = await api("/api/backups", { method: "POST" });
  const panel = $("backup-result");
  panel.replaceChildren(
    el("p", "", `已保存 ${value.object_count} 份原件 · 提交位置 ${value.commit_seq}`),
    button("下载备份", () =>
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
  $("create-backup").onclick = () => busy($("create-backup"), createBackup);
  setInterval(() => {
    if (state.token && location.hash === "#jobs") loadJobs().catch(() => {});
  }, 5000);
}
