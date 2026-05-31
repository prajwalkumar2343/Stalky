import json
import subprocess
import textwrap
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
EXTENSION_DIR = REPO_ROOT / "apps" / "macos" / "BrowserExtension" / "chromium"
RESOURCE_EXTENSION_DIR = (
    REPO_ROOT
    / "apps"
    / "macos"
    / "Sources"
    / "AuraBot"
    / "Resources"
    / "BrowserExtension"
    / "chromium"
)
SERVER_SOURCE = (
    REPO_ROOT
    / "apps"
    / "macos"
    / "Sources"
    / "AuraBot"
    / "Services"
    / "BrowserExtensionServer.swift"
)
CONTEXT_ROUTER_SOURCE = (
    REPO_ROOT
    / "apps"
    / "macos"
    / "Sources"
    / "AuraBot"
    / "ContextRouting"
    / "ContextRouter.swift"
)


def run_background_helper(expression):
    script = textwrap.dedent(
        f"""
        const fs = require("node:fs");
        const vm = require("node:vm");
        const assert = require("node:assert/strict");
        const extensionDir = {json.dumps(str(EXTENSION_DIR))};
        const context = {{
          console,
          URL,
          Set,
          Boolean,
          Number,
          String,
          Date,
          importScripts(file) {{
            vm.runInContext(
              fs.readFileSync(`${{extensionDir}}/${{file}}`, "utf8"),
              context,
              {{ filename: file }}
            );
          }},
          chrome: {{
            runtime: {{
              onInstalled: {{ addListener() {{}} }},
              onMessage: {{ addListener() {{}} }}
            }},
            tabs: {{
              onActivated: {{ addListener() {{}} }},
              onUpdated: {{ addListener() {{}} }}
            }},
            storage: {{ sync: {{ get: async () => ({{}}), set: async () => {{}} }} }}
          }}
        }};
        vm.createContext(context);
        vm.runInContext(
          fs.readFileSync(`${{extensionDir}}/background.js`, "utf8"),
          context,
          {{ filename: "background.js" }}
        );
        {expression}
        """
    )
    return subprocess.run(
        ["node", "-e", script],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )


def run_options_helper(expression):
    script = textwrap.dedent(
        f"""
        const fs = require("node:fs");
        const vm = require("node:vm");
        const assert = require("node:assert/strict");
        const extensionDir = {json.dumps(str(EXTENSION_DIR))};
        const fieldValues = new Map();
        const context = {{
          console,
          URL,
          Set,
          String,
          Boolean,
          setTimeout(callback) {{ callback(); }},
          document: {{
            getElementById(id) {{
              if (!fieldValues.has(id)) {{
                fieldValues.set(id, {{
                  value: "",
                  checked: false,
                  textContent: "",
                  addEventListener() {{}}
                }});
              }}
              return fieldValues.get(id);
            }}
          }},
          chrome: {{
            storage: {{ sync: {{ get: async (defaults) => defaults, set: async () => {{}} }} }}
          }}
        }};
        vm.createContext(context);
        vm.runInContext(
          fs.readFileSync(`${{extensionDir}}/settings.js`, "utf8"),
          context,
          {{ filename: "settings.js" }}
        );
        vm.runInContext(
          fs.readFileSync(`${{extensionDir}}/options.js`, "utf8"),
          context,
          {{ filename: "options.js" }}
        );
        {expression}
        """
    )
    return subprocess.run(
        ["node", "-e", script],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )


class BrowserExtensionContractsTests(unittest.TestCase):
    def test_bundled_background_matches_development_copy(self):
        self.assertEqual(
            (EXTENSION_DIR / "background.js").read_text(),
            (RESOURCE_EXTENSION_DIR / "background.js").read_text(),
        )

    def test_bundled_options_matches_development_copy(self):
        self.assertEqual(
            (EXTENSION_DIR / "options.js").read_text(),
            (RESOURCE_EXTENSION_DIR / "options.js").read_text(),
        )

    def test_background_sanitize_context_drops_private_text_and_clamps_values(self):
        run_background_helper(
            """
            const contextValue = context.sanitizeContext({
              browser: "  ",
              url: "  https://Example.com/docs/page?utm=1#intro  ",
              title: "  Example   Docs  ",
              visibleText: "  private visible text  ",
              selectedText: " private selected ",
              readableText: " private readable ",
              scrollPercent: 250,
              noveltyScore: -5,
              textCaptureMode: " visible_viewport "
            }, {
              incognito: true,
              url: "https://fallback.example/path",
              title: "Fallback title"
            });

            assert.equal(contextValue.schemaVersion, 1);
            assert.equal(contextValue.browser, "Chromium Browser");
            assert.equal(contextValue.title, "Example Docs");
            assert.equal(contextValue.pageID, "example.com/docs/page");
            assert.equal(contextValue.scrollPercent, 100);
            assert.equal(contextValue.noveltyScore, 0);
            assert.equal(contextValue.textCaptureMode, "private_window_metadata_only");
            assert.equal(contextValue.visibleText, undefined);
            assert.equal(contextValue.selectedText, undefined);
            assert.equal(contextValue.readableText, undefined);
            """
        )

    def test_background_sanitize_context_drops_metadata_only_text(self):
        run_background_helper(
            """
            const contextValue = context.sanitizeContext({
              browser: "Chrome",
              textCaptureMode: "sensitive_metadata_only",
              visibleText: "secret",
              selectedText: "secret",
              readableText: "secret",
              visibleTextHash: " visible-hash ",
              readableTextHash: " readable-hash "
            }, { incognito: false });

            assert.equal(contextValue.visibleText, undefined);
            assert.equal(contextValue.selectedText, undefined);
            assert.equal(contextValue.readableText, undefined);
            assert.equal(contextValue.visibleTextHash, "visible-hash");
            assert.equal(contextValue.readableTextHash, "readable-hash");
            """
        )

    def test_background_context_endpoint_is_local_only(self):
        run_background_helper(
            """
            assert.equal(
              context.contextEndpoint("http://localhost:9999/custom/"),
              "http://localhost:9999/custom/browser/context"
            );
            assert.equal(
              context.contextEndpoint("https://evil.example/collect"),
              "http://127.0.0.1:7345/browser/context"
            );
            assert.equal(
              context.contextEndpoint("not a url"),
              "http://127.0.0.1:7345/browser/context"
            );
            """
        )

    def test_options_normalizes_server_url_before_save(self):
        run_options_helper(
            """
            assert.equal(
              JSON.stringify(context.normalizeLocalServerURL("http://localhost:9999/custom/?token=bad#frag")),
              JSON.stringify({ value: "http://localhost:9999/custom", wasReset: false })
            );
            assert.equal(
              JSON.stringify(context.normalizeLocalServerURL("https://evil.example/collect")),
              JSON.stringify({ value: "http://127.0.0.1:7345", wasReset: true })
            );
            assert.equal(
              JSON.stringify(context.normalizeLocalServerURL("not a url")),
              JSON.stringify({ value: "http://127.0.0.1:7345", wasReset: true })
            );
            """
        )

    def test_server_cors_allows_documented_fallback_api_key_header(self):
        source = SERVER_SOURCE.read_text()

        self.assertIn('HTTPHeaders.Name("X-AuraBot-Extension-Key")', source)
        self.assertIn('headers.first(name: "X-AuraBot-Extension-Key")', source)

    def test_status_endpoint_never_reports_negative_context_age(self):
        source = SERVER_SOURCE.read_text()

        self.assertIn("max(0, Date().timeIntervalSince($0.timestamp))", source)

    def test_context_router_browser_fingerprint_tracks_content_changes(self):
        source = CONTEXT_ROUTER_SOURCE.read_text()

        for required_component in (
            "visibleTextHash",
            "readableTextHash",
            "textCaptureMode",
            "sourceQuality.rawValue",
            "privateWindow",
        ):
            self.assertIn(required_component, source)


if __name__ == "__main__":
    unittest.main()
