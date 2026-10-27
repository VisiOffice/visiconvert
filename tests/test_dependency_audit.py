import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('audit', Path(__file__).resolve().parents[1] / 'scripts/audit-dependencies.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AdvisoryReviewTest(unittest.TestCase):
    def test_only_reviewed_unaffected_lock_entry_is_excepted(self):
        finding = ('crates.io', 'block-buffer', '0.10.4', 'GHSA-qwgh-2vcv-g2f7')
        self.assertTrue(audit.reviewed_nonapplicable(finding))
        for index, replacements in [(0, ['npm']), (1, ['other-package']),
                                    (2, ['0.10.3', '0.11.0', '0.12.0', '0.12.1']),
                                    (3, ['GHSA-another-advisory'])]:
            for replacement in replacements:
                other = list(finding)
                other[index] = replacement
                self.assertFalse(audit.reviewed_nonapplicable(tuple(other)))


if __name__ == '__main__':
    unittest.main()
