import { $, state, el, api, subjectPath, op, form, notify, busy, jsonView } from "./client.js";
import { t } from "./i18n.js";

let bundle;

function saveFile(content, filename) {
  const url = URL.createObjectURL(new Blob([content], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}

async function exportBundle() {
  const value = await api(subjectPath("/bundles/export"), { method: "POST", body: {} });
  saveFile(JSON.stringify(value), `nous-bundle-${state.subject}.json`);
  notify("web-bundle-exported");
}

export function clearBundle() {
  bundle = undefined;
  $("bundle-file").value = "";
  $("bundle-preview").textContent = "";
  $("bundle-result").replaceChildren();
}

async function inspectBundle() {
  bundle = undefined;
  $("bundle-result").replaceChildren();
  const file = $("bundle-file").files[0];
  if (!file) return;
  if (file.size > 64 * 1024 * 1024) throw new Error(t("web-bundle-size"));
  const value = JSON.parse(await file.text());
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(t("web-bundle-invalid"));
  bundle = value;
  $("bundle-preview").textContent = t("web-bundle-ready", {
    filename: file.name,
    bytes: file.size,
  });
}

async function installBundle() {
  if (!bundle) throw new Error(t("web-bundle-select"));
  if (!window.confirm(t("web-bundle-confirm"))) return;
  const value = await api(subjectPath("/bundles/install"), {
    method: "POST",
    body: { operation_id: op(), bundle },
  });
  $("bundle-result").replaceChildren(jsonView(value));
  notify("web-bundle-installed");
}

async function restoreInstance() {
  const file = $("restore-file").files[0];
  if (!file) throw new Error(t("web-backup-select"));
  const params = new URLSearchParams({ instance_name: $("restore-name").value });
  const value = await api(`/api/backups/restore?${params}`, {
    method: "POST",
    headers: { "Content-Type": "application/zip" },
    body: file,
  });
  $("restore-result").replaceChildren(
    el("h3", "", t("web-restored")),
    el("p", "", t("web-restored-help", { path: value.instance })),
    el("p", "", t("web-restored-credential", { path: value.credential_file })),
  );
  notify("web-restored");
}

export function initializeBundles() {
  $("export-bundle").onclick = () => busy($("export-bundle"), exportBundle);
  $("bundle-file").onchange = () => busy($("bundle-file"), inspectBundle);
  form("bundle-form", installBundle);
  form("restore-form", restoreInstance);
}
