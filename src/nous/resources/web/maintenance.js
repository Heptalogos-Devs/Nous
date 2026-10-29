import { t, message, date } from "./i18n.js";
import { $, state, el, api, op, button, jsonView, subjectPath, form } from "./client.js";

let runs = [],
  schedules = [],
  current = null;
const base = () => subjectPath("/maintenance");
function heading(key, tag = "h2") {
  return message(el(tag), key);
}
function input(id, key, type = "text") {
  const label = el("label"),
    field = el(type === "textarea" ? "textarea" : "input");
  field.id = id;
  if (type !== "textarea") field.type = type;
  label.append(heading(key, "span"), field);
  return label;
}
function options(prefix = "maintenance") {
  const limit = Number($(`${prefix}-limit`).value);
  if (!Number.isInteger(limit) || limit < 1 || limit > 1000)
    throw new Error(t("web-maintenance-limit-invalid"));
  return { instruction: $(`${prefix}-instruction`).value, limit };
}
export function clearMaintenance() {
  runs = [];
  schedules = [];
  current = null;
  $("maintenance-runs").replaceChildren();
  $("maintenance-schedules").replaceChildren();
  $("maintenance-detail").replaceChildren();
}
export async function loadMaintenance() {
  [runs, schedules] = await Promise.all([
    api(base() + "/runs?limit=50"),
    api(base() + "/schedules"),
  ]);
  if (current) current = await api(base() + `/runs/${encodeURIComponent(current.id)}`);
  renderMaintenance();
}
async function openRun(id) {
  current = await api(base() + `/runs/${encodeURIComponent(id)}`);
  renderMaintenance();
  const identity = current.id;
  const detail = await api(
    `/api/jobs/${encodeURIComponent(current.job_id)}?` +
      new URLSearchParams({ namespace_id: state.namespace, subject_id: state.subject }),
  );
  if (current?.id !== identity) return;
  $("maintenance-job").replaceChildren(el("p", "", `${detail.job.status} · ${current.job_id}`));
  if (detail.job.error) $("maintenance-job").append(jsonView(detail.job.error));
}
function fact(run) {
  if (run.purged_at) return t("web-maintenance-purged");
  if (run.review)
    return t(
      run.review.decision === "adopt" ? "web-maintenance-adopted" : "web-maintenance-rejected",
    );
  return t(run.draft ? "web-maintenance-proposed" : "web-maintenance-submitted");
}
export function renderMaintenance() {
  const list = $("maintenance-runs");
  list.replaceChildren(heading("web-maintenance-runs"));
  for (const run of runs)
    list.append(
      button(`${date(run.created_at)} · ${fact(run)} · ${run.id.slice(0, 8)}`, () =>
        openRun(run.id),
      ),
    );
  if (!runs.length) list.append(heading("web-maintenance-no-runs", "p"));
  const schedulesPanel = $("maintenance-schedules");
  schedulesPanel.replaceChildren(heading("web-maintenance-schedules"));
  for (const schedule of schedules) {
    const row = el("div", "form-sheet");
    row.append(
      el("h3", "", schedule.name),
      el("p", "ref", `${schedule.cron} · ${schedule.timezone}`),
    );
    if (schedule.stopped_at)
      row.append(el("p", "", `${t("web-maintenance-stopped")} · ${date(schedule.stopped_at)}`));
    else
      row.append(
        button(t("web-maintenance-stop"), async () => {
          await api(base() + `/schedules/${encodeURIComponent(schedule.id)}/stop`, {
            method: "POST",
            body: { operation_id: op() },
          });
          await loadMaintenance();
        }),
      );
    schedulesPanel.append(row);
  }
  const panel = $("maintenance-detail");
  panel.replaceChildren();
  if (!current) return;
  panel.append(el("h2", "", fact(current)), el("div", "ref", current.id));
  panel.append(Object.assign(el("div"), { id: "maintenance-job" }));
  panel.append(button(t("web-maintenance-job"), () => openRun(current.id)));
  if (current.purged_at || current.sources_available === false) {
    panel.append(
      heading(
        current.purged_at ? "web-maintenance-purged" : "web-maintenance-source-unavailable",
        "p",
      ),
    );
    return;
  }
  if (current.draft) {
    panel.append(
      el("h3", "", current.draft.summary),
      el("p", "body-text", current.draft.rationale),
    );
    for (const action of current.draft.actions) {
      const item = el("section", "form-sheet");
      const label = {
        adopt: "web-maintenance-action-adopt",
        merge_concepts: "web-maintenance-action-merge",
        split_concept: "web-maintenance-action-split",
        memory_change: "web-maintenance-action-memory",
      }[action.action];
      item.append(el("h3", "", `${action.key} · ${t(label)}`), el("p", "", action.reason));
      if (action.target_key)
        item.append(el("p", "ref", `${t("web-maintenance-revise")}: ${action.target_key}`));
      if (action.action === "adopt") {
        item.append(
          el("p", "ref", `${t("web-maintenance-evidence")}: ${action.evidence_keys.join(", ")}`),
        );
        item.append(jsonView(action.body_input));
        for (const binding of action.bindings)
          item.append(el("p", "ref", `${binding.path} → ${binding.key}`));
      } else if (action.action === "merge_concepts") {
        item.append(el("p", "ref", `${action.source_keys.join(", ")} → ${action.target_key}`));
      } else if (action.action === "split_concept") {
        item.append(el("p", "ref", `${t("web-maintenance-revise")}: ${action.source_key}`));
        for (const partition of action.partitions)
          item.append(el("p", "", `${partition.label} · ${partition.member_keys.join(", ")}`));
      } else if (action.action === "memory_change") {
        item.append(
          el(
            "p",
            "",
            `${action.transition} · ${t("web-maintenance-strength")}: ${action.strength}`,
          ),
        );
        if (action.hours !== null)
          item.append(el("p", "", `${t("web-maintenance-hours")}: ${action.hours}`));
      }
      panel.append(item);
    }
    if (current.catalog) {
      const references = el("details");
      references.append(heading("web-maintenance-snapshot", "summary"));
      for (const entry of current.catalog.entries)
        references.append(
          el(
            "p",
            "ref",
            `${entry.key} → ${entry.reference.kind}:${entry.reference.id}@${entry.reference.revision}`,
          ),
        );
      panel.append(references);
    }
    if (!current.review) {
      const review = el("form", "form-sheet");
      review.id = "maintenance-review";
      review.append(input("maintenance-reason", "web-reason", "textarea"));
      const buttons = el("div", "button-row");
      for (const [decision, key] of [
        ["adopt", "web-maintenance-approve"],
        ["reject", "web-maintenance-reject"],
      ]) {
        const node = message(el("button", decision === "reject" ? "secondary" : ""), key);
        node.type = "submit";
        node.value = decision;
        buttons.append(node);
      }
      review.append(buttons);
      panel.append(review);
      form("maintenance-review", async (event) => {
        const reviewed = await api(base() + `/runs/${encodeURIComponent(current.id)}/review`, {
          method: "POST",
          body: {
            operation_id: op(),
            expected_revision: current.revision,
            decision: event.submitter.value,
            reason: $("maintenance-reason").value,
          },
        });
        current = reviewed;
        await loadMaintenance();
      });
    }
  }
  if (current.review) {
    panel.append(
      el("p", "", `${current.review.caller_id} · ${date(current.review.recorded_at)}`),
      el("p", "", current.review.reason),
    );
    for (const ref of current.review.adopted)
      panel.append(el("p", "ref", `${ref.kind}:${ref.id}@${ref.revision}`));
  }
}
export function initializeMaintenance() {
  const controls = $("maintenance-controls");
  const submit = el("form", "form-sheet");
  submit.id = "maintenance-submit";
  submit.append(
    input("maintenance-instruction", "web-maintenance-instruction", "textarea"),
    input("maintenance-limit", "web-maintenance-limit", "number"),
  );
  $("maintenance-controls").append(submit);
  $("maintenance-limit").value = "500";
  $("maintenance-limit").min = "1";
  $("maintenance-limit").max = "1000";
  submit.append(message(el("button"), "web-maintenance-start"));
  const schedule = el("form", "form-sheet");
  schedule.id = "maintenance-schedule";
  schedule.append(
    heading("web-maintenance-schedules"),
    input("maintenance-schedule-name", "web-name"),
    input("maintenance-cron", "web-maintenance-cron"),
    input("maintenance-timezone", "web-maintenance-timezone"),
    input("maintenance-schedule-instruction", "web-maintenance-instruction", "textarea"),
    input("maintenance-schedule-limit", "web-maintenance-limit", "number"),
    heading("web-maintenance-schedule-help", "p"),
    message(el("button"), "web-maintenance-create-schedule"),
  );
  controls.append(schedule);
  $("maintenance-schedule-limit").value = "500";
  $("maintenance-schedule-limit").min = "1";
  $("maintenance-schedule-limit").max = "1000";
  $("maintenance-cron").value = "0 0 * * *";
  $("maintenance-timezone").value = "UTC";
  for (const id of ["maintenance-schedule-name", "maintenance-cron", "maintenance-timezone"])
    $(id).required = true;
  controls.append(message(button(t("web-refresh"), loadMaintenance), "web-refresh"));
  form("maintenance-submit", async () => {
    const result = await api(base() + "/runs", {
      method: "POST",
      body: { operation_id: op(), ...options() },
    });
    await loadMaintenance();
    await openRun(result.run_id);
  });
  form("maintenance-schedule", async () => {
    await api(base() + "/schedules", {
      method: "POST",
      body: {
        operation_id: op(),
        name: $("maintenance-schedule-name").value,
        cron: $("maintenance-cron").value,
        timezone: $("maintenance-timezone").value,
        command: options("maintenance-schedule"),
      },
    });
    await loadMaintenance();
  });
}
