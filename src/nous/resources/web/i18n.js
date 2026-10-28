import { FluentBundle, FluentResource } from "./fluent.js";

const available = ["en-US", "zh-CN"];
let bundle;
export let locale = "en-US";
const catalogs = new Map();
let generation = 0;

export function t(key, args = {}) {
  const message = bundle?.getMessage(key);
  if (!message?.value) throw new Error(`Missing translation: ${key}`);
  const errors = [];
  const value = bundle.formatPattern(message.value, args, errors);
  if (errors.length) throw errors[0];
  return value;
}

export function translate(root = document) {
  for (const node of root.querySelectorAll("[data-i18n]"))
    node.textContent = t(node.dataset.i18n, JSON.parse(node.dataset.i18nArgs || "{}"));
  for (const attribute of ["placeholder", "aria-label", "title"])
    for (const node of root.querySelectorAll(`[data-i18n-${attribute}]`))
      node.setAttribute(attribute, t(node.getAttribute(`data-i18n-${attribute}`)));
}

export function message(node, key, args = {}) {
  node.dataset.i18n = key;
  node.dataset.i18nArgs = JSON.stringify(args);
  node.textContent = t(key, args);
  return node;
}

export async function setLocale(preferred) {
  const requested = ++generation;
  const selected = available.includes(preferred) ? preferred : "en-US";
  if (!catalogs.has(selected)) {
    const response = await fetch(`/assets/locales/${selected}.ftl`);
    if (!response.ok) throw new Error("Language resources unavailable");
    const next = new FluentBundle(selected, { useIsolating: false });
    const errors = next.addResource(new FluentResource(await response.text()));
    if (errors.length) throw errors[0];
    catalogs.set(selected, next);
  }
  if (requested !== generation) return;
  bundle = catalogs.get(selected);
  locale = selected;
  localStorage.setItem("nous.locale", selected);
  document.documentElement.lang = selected;
  translate();
  document.querySelectorAll("[data-locale]").forEach((node) => (node.value = selected));
  window.dispatchEvent(new Event("nous:locale"));
}

export async function initializeLocale() {
  const preferred =
    localStorage.getItem("nous.locale") ||
    (navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US");
  await setLocale(preferred);
  for (const node of document.querySelectorAll("[data-locale]"))
    node.addEventListener("change", () => setLocale(node.value).catch(console.error));
}

export function number(value, options = {}) {
  return new Intl.NumberFormat(locale, options).format(value);
}

export function date(value) {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "UTC",
  }).format(new Date(value));
}
