importScripts("settings.js");

const CONTEXT_SCHEMA_VERSION = 1;

chrome.runtime.onInstalled.addListener(async () => {
  const existing = await chrome.storage.sync.get(DEFAULT_SETTINGS);
  await chrome.storage.sync.set({ ...DEFAULT_SETTINGS, ...existing });
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type !== "AURABOT_CONTEXT") {
    return false;
  }

  postContext(message.context, sender.tab)
    .then(() => sendResponse({ ok: true }))
    .catch((error) => sendResponse({ ok: false, error: String(error) }));

  return true;
});

chrome.tabs.onActivated.addListener(({ tabId }) => {
  requestTabContext(tabId, "tab_activated");
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "complete" || changeInfo.url || changeInfo.title) {
    requestTabContext(tabId, "tab_updated");
  }
});

async function requestTabContext(tabId, reason) {
  try {
    await chrome.tabs.sendMessage(tabId, {
      type: "AURABOT_COLLECT_CONTEXT",
      reason
    });
  } catch {
    // The tab may not have a content script, e.g. browser settings pages.
  }
}

async function postContext(rawContext, tab) {
  const settings = await chrome.storage.sync.get(DEFAULT_SETTINGS);
  if (!settings.captureEnabled) {
    return;
  }

  const context = sanitizeContext(rawContext, tab);
  const endpoint = contextEndpoint(settings.serverURL);
  const headers = {
    "Content-Type": "application/json"
  };

  const apiKey = normalizeOptionalString(settings.apiKey);
  if (apiKey) {
    headers.Authorization = `Bearer ${apiKey}`;
  }

  const response = await fetch(endpoint, {
    method: "POST",
    headers,
    body: JSON.stringify(context)
  });

  if (!response.ok) {
    throw new Error(`AuraBot context update failed: ${response.status}`);
  }
}

function sanitizeContext(context, tab) {
  const rawContext = context && typeof context === "object" ? context : {};
  const privateWindow = Boolean(tab?.incognito);
  const tabURL = normalizeOptionalString(tab?.url);
  const tabTitle = normalizeOptionalString(tab?.title);
  const textCaptureMode = normalizeOptionalString(rawContext.textCaptureMode);
  const normalizedTextCaptureMode = String(textCaptureMode || "").toLowerCase();
  const shouldDropText =
    privateWindow ||
    normalizedTextCaptureMode.includes("metadata_only") ||
    normalizedTextCaptureMode.includes("sensitive");

  const safeContext = {
    ...rawContext,
    schemaVersion: CONTEXT_SCHEMA_VERSION,
    privateWindow,
    browser: normalizeOptionalString(rawContext.browser) || "Chromium Browser",
    bundleIdentifier: normalizeOptionalString(rawContext.bundleIdentifier),
    url: normalizeOptionalString(rawContext.url) || tabURL,
    title: normalizeOptionalString(rawContext.title) || tabTitle,
    pageID: normalizeOptionalString(rawContext.pageID) || normalizedPageIDFromURL(rawContext.url || tabURL),
    mediaID: normalizeOptionalString(rawContext.mediaID),
    viewportSignature: normalizeOptionalString(rawContext.viewportSignature),
    scrollPercent: boundedNumber(rawContext.scrollPercent, 0, 100),
    noveltyScore: boundedNumber(rawContext.noveltyScore, 0, 1),
    visibleTextHash: normalizeOptionalString(rawContext.visibleTextHash),
    readableTextHash: normalizeOptionalString(rawContext.readableTextHash),
    textCaptureMode: privateWindow ? "private_window_metadata_only" : textCaptureMode,
    timestamp: new Date().toISOString()
  };

  safeContext.visibleText = shouldDropText ? undefined : trimmedString(rawContext.visibleText, 8 * 1024);
  safeContext.selectedText = shouldDropText ? undefined : trimmedString(rawContext.selectedText, 2 * 1024);
  safeContext.readableText = shouldDropText ? undefined : trimmedString(rawContext.readableText, 64 * 1024);

  if (shouldDropText) {
    delete safeContext.visibleText;
    delete safeContext.selectedText;
    delete safeContext.readableText;
  }

  return safeContext;
}

function contextEndpoint(serverURL) {
  const normalized = normalizeOptionalString(serverURL) || DEFAULT_SETTINGS.serverURL;
  let url;
  try {
    url = new URL(normalized);
  } catch {
    url = new URL(DEFAULT_SETTINGS.serverURL);
  }

  const localHosts = new Set(["127.0.0.1", "localhost", "[::1]"]);
  if (url.protocol !== "http:" || !localHosts.has(url.hostname)) {
    url = new URL(DEFAULT_SETTINGS.serverURL);
  }

  url.pathname = `${url.pathname.replace(/\/+$/, "")}/browser/context`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

function normalizeOptionalString(value) {
  if (value === undefined || value === null) {
    return undefined;
  }
  const normalized = String(value).replace(/\s+/g, " ").trim();
  return normalized || undefined;
}

function trimmedString(value, maxLength) {
  const normalized = normalizeOptionalString(value);
  if (!normalized) {
    return undefined;
  }
  return normalized.length > maxLength ? normalized.slice(0, maxLength) : normalized;
}

function boundedNumber(value, min, max) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return undefined;
  }
  return Math.max(min, Math.min(max, number));
}

function normalizedPageIDFromURL(value) {
  const normalized = normalizeOptionalString(value);
  if (!normalized) {
    return undefined;
  }

  try {
    const url = new URL(normalized);
    return `${url.hostname.toLowerCase()}${url.pathname || "/"}`;
  } catch {
    return undefined;
  }
}
