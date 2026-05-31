const fields = {
  captureEnabled: document.getElementById("captureEnabled"),
  serverURL: document.getElementById("serverURL"),
  apiKey: document.getElementById("apiKey"),
  captureFullPageText: document.getElementById("captureFullPageText"),
  disabledDomains: document.getElementById("disabledDomains")
};
const status = document.getElementById("status");

restore();
document.getElementById("save").addEventListener("click", save);

async function restore() {
  const settings = await chrome.storage.sync.get(DEFAULT_SETTINGS);
  fields.captureEnabled.checked = Boolean(settings.captureEnabled);
  fields.serverURL.value = settings.serverURL || DEFAULT_SETTINGS.serverURL;
  fields.apiKey.value = settings.apiKey || "";
  fields.captureFullPageText.checked = Boolean(settings.captureFullPageText);
  fields.disabledDomains.value = settings.disabledDomains || "";
}

async function save() {
  const normalizedServerURL = normalizeLocalServerURL(fields.serverURL.value);
  await chrome.storage.sync.set({
    captureEnabled: fields.captureEnabled.checked,
    serverURL: normalizedServerURL.value,
    apiKey: fields.apiKey.value.trim(),
    captureFullPageText: fields.captureFullPageText.checked,
    disabledDomains: fields.disabledDomains.value.trim()
  });

  fields.serverURL.value = normalizedServerURL.value;
  status.textContent = normalizedServerURL.wasReset
    ? "Saved with the default local Aura endpoint."
    : "Saved.";
  setTimeout(() => {
    status.textContent = "";
  }, 1800);
}

function normalizeLocalServerURL(value) {
  const normalized = String(value || "").trim() || DEFAULT_SETTINGS.serverURL;
  let url;
  try {
    url = new URL(normalized);
  } catch {
    return { value: DEFAULT_SETTINGS.serverURL, wasReset: true };
  }

  const localHosts = new Set(["127.0.0.1", "localhost", "[::1]"]);
  if (url.protocol !== "http:" || !localHosts.has(url.hostname)) {
    return { value: DEFAULT_SETTINGS.serverURL, wasReset: true };
  }

  url.pathname = url.pathname.replace(/\/+$/, "");
  url.search = "";
  url.hash = "";
  return { value: url.toString(), wasReset: false };
}
