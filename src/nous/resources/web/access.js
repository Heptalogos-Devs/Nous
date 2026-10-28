import { $, state, el, api, op, table, button, form, notify, busy, chip } from "./client.js";
import { t, date } from "./i18n.js";

export function clearIssuedToken() {
  $("issued-token").value = "";
  $("issued-token").type = "password";
  $("issued-credential").hidden = true;
}

function scopeText(record) {
  return [
    ["web-namespace", record.namespace_ids],
    ["web-subject", record.subject_ids],
    ["web-sources", record.source_ids],
  ]
    .map(
      ([key, ids]) =>
        `${t(key)}: ${ids === null ? t("web-all-granted") : ids.join(", ") || t("web-none")}`,
    )
    .join("\n");
}

export async function loadCredentials() {
  const records = await api("/api/credentials");
  const rows = records.map((record) => [
    record.name,
    record.capabilities.map((capability) => t(`web-cap-${capability}`)).join(", "),
    scopeText(record),
    date(record.created_at),
    record.revoked_at
      ? chip(t("web-revoked"), true)
      : button(
          t("web-revoke"),
          async () => {
            if (!window.confirm(t("web-revoke-confirm", { name: record.name }))) return;
            await api(`/api/credentials/${encodeURIComponent(record.id)}?operation_id=${op()}`, {
              method: "DELETE",
            });
            notify("web-credential-revoked");
            await loadCredentials();
          },
          "danger",
        ),
  ]);
  const container = el("div", "table-scroll");
  container.append(
    records.length
      ? table(
          [
            t("web-name"),
            t("web-capabilities"),
            t("web-credential-scope"),
            t("web-created"),
            t("web-actions"),
          ],
          rows,
        )
      : el("div", "empty", t("web-no-credentials")),
  );
  $("credential-list").replaceChildren(container);
}

async function createCredential() {
  clearIssuedToken();
  const scope = $("credential-scope").value;
  const command = {
    operation_id: op(),
    name: $("credential-name").value,
    capabilities: [...document.querySelectorAll("input[name=capability]:checked")].map(
      (node) => node.value,
    ),
  };
  if (!command.capabilities.length) throw new Error(t("web-select-capability"));
  if (scope !== "inherit") {
    if (!state.namespace || (scope === "subject" && !state.subject))
      throw new Error(t("web-select-scope"));
    command.namespace_ids = [state.namespace];
    if (scope === "subject") command.subject_ids = [state.subject];
  }
  const sources = $("credential-sources")
    .value.split("\n")
    .map((value) => value.trim())
    .filter(Boolean);
  if (sources.length) command.source_ids = sources;
  const value = await api("/api/credentials", { method: "POST", body: command });
  if (value.token) {
    $("issued-token").value = value.token;
    $("issued-credential").hidden = false;
  }
  $("credential-name").value = "";
  notify("web-credential-created");
  await loadCredentials();
  $("issued-credential").scrollIntoView({ block: "nearest", behavior: "smooth" });
}

export function initializeAccess() {
  form("credential-form", createCredential);
  $("refresh-credentials").onclick = () => busy($("refresh-credentials"), loadCredentials);
  $("dismiss-token").onclick = clearIssuedToken;
  $("reveal-token").onclick = () => {
    $("issued-token").type = $("issued-token").type === "password" ? "text" : "password";
  };
  $("copy-token").onclick = () =>
    busy($("copy-token"), async () => {
      await navigator.clipboard.writeText($("issued-token").value);
      notify("web-token-copied");
    });
}
