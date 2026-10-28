export const $ = (id) => document.getElementById(id);
export const state = {
  token: "",
  namespace: localStorage.getItem("nous.namespace") || "",
  subject: localStorage.getItem("nous.subject") || "",
  config: null,
  preview: null,
};
export function el(tag, cls = "", text = "") {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== "") node.textContent = String(text);
  return node;
}
export function jsonView(value) {
  return el("pre", "json-view", JSON.stringify(value, null, 2));
}
export function notice(message, failed = false) {
  $("notice").hidden = false;
  $("notice").classList.toggle("error", failed);
  $("notice").textContent = message;
}
export function error(error) {
  notice(error.message || String(error), true);
}
export async function api(path, options = {}) {
  const headers = new Headers(options.headers || {});
  headers.set("Authorization", `Bearer ${state.token}`);
  let body = options.body;
  if (body !== undefined && !(body instanceof Blob) && typeof body !== "string") {
    headers.set("Content-Type", "application/json");
    body = JSON.stringify(body);
  }
  const response = await fetch(path, { ...options, headers, body });
  const value = response.headers.get("content-type")?.includes("json")
    ? await response.json()
    : await response.text();
  if (!response.ok)
    throw new Error(value?.error?.message || value?.detail || `请求失败 (${response.status})`);
  return value;
}
export function subjectPath(suffix = "") {
  if (!state.namespace || !state.subject) throw new Error("请先选择命名空间和主体");
  return `/api/namespaces/${encodeURIComponent(state.namespace)}/subjects/${encodeURIComponent(state.subject)}${suffix}`;
}
export function op() {
  return crypto.randomUUID();
}
export function table(headers, rows) {
  const node = el("table", "data-table");
  const head = el("thead");
  const tr = el("tr");
  headers.forEach((text) => tr.append(el("th", "", text)));
  head.append(tr);
  node.append(head);
  const body = el("tbody");
  rows.forEach((cells) => {
    const row = el("tr");
    cells.forEach((value) => {
      const td = el("td");
      td.append(value instanceof Node ? value : document.createTextNode(String(value ?? "—")));
      row.append(td);
    });
    body.append(row);
  });
  node.append(body);
  return node;
}
export function button(text, action, cls = "secondary") {
  const node = el("button", cls, text);
  node.type = "button";
  node.addEventListener("click", () => busy(node, action));
  return node;
}
export async function busy(node, action) {
  node.disabled = true;
  try {
    await action();
  } catch (e) {
    error(e);
  } finally {
    node.disabled = false;
  }
}
export function form(id, action) {
  $(id).addEventListener("submit", (event) => {
    event.preventDefault();
    const submit = event.submitter || $(id).querySelector("button[type=submit],button:not([type])");
    busy(submit, () => action(event));
  });
}
export function chip(text, inactive = false) {
  return el("span", `chip${inactive ? " inactive" : ""}`, text);
}
export async function download(path, filename) {
  const response = await fetch(path, { headers: { Authorization: `Bearer ${state.token}` } });
  if (!response.ok) throw new Error(`下载失败 (${response.status})`);
  const url = URL.createObjectURL(await response.blob());
  const link = el("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}
