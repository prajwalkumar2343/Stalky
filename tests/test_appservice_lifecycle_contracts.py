import re
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
APP_SERVICE_SOURCE = (
    REPO_ROOT
    / "apps"
    / "macos"
    / "Sources"
    / "AuraBot"
    / "Services"
    / "AppService.swift"
)


class AppServiceLifecycleContractsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = APP_SERVICE_SOURCE.read_text()

    def test_start_is_idempotent_to_avoid_background_task_loops(self):
        start_body = self._function_body("start")

        self.assertIn("guard status != .running", start_body)
        self.assertIn("startHealthPolling()", start_body)

    def test_health_polling_has_explicit_cancel_path(self):
        self.assertIn("private var healthCheckTask: Task<Void, Never>?", self.source)
        self.assertIn("healthCheckTask?.cancel()", self._function_body("startHealthPolling"))
        self.assertIn("healthCheckTask?.cancel()", self._function_body("stopHealthPolling"))
        self.assertIn("stopHealthPolling()", self._function_body("stop"))

    def test_configuration_is_sanitized_before_live_services_are_rebuilt(self):
        save_body = self._function_body("saveConfiguration")
        apply_body = self._function_body("applyConfiguration")
        enable_computer_use_body = self._function_body("enableComputerUse")

        self.assertIn("let sanitizedConfig = newConfig.sanitizedForPersistence()", save_body)
        self.assertIn("try sanitizedConfig.save", save_body)
        self.assertIn("await applyConfiguration(sanitizedConfig)", save_body)
        self.assertIn("let newConfig = newConfig.sanitizedForPersistence()", apply_body)
        self.assertIn("updatedConfig = updatedConfig.sanitizedForPersistence()", enable_computer_use_body)

    def _function_body(self, name):
        match = re.search(rf"\bfunc {name}\([^)]*\) [^{{]*\{{", self.source)
        self.assertIsNotNone(match, f"Could not find function {name}")

        index = match.end()
        depth = 1
        while index < len(self.source) and depth > 0:
            if self.source[index] == "{":
                depth += 1
            elif self.source[index] == "}":
                depth -= 1
            index += 1

        return self.source[match.end(): index - 1]


if __name__ == "__main__":
    unittest.main()
