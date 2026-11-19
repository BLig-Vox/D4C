import importlib.util
import unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('catalog',Path(__file__).resolve().parents[1]/'scripts/sync-codex-models.py')
catalog=importlib.util.module_from_spec(spec);spec.loader.exec_module(catalog)

class CatalogTests(unittest.TestCase):
    def test_removes_unsupported_codex_entries_but_preserves_other_runtimes(self):
        config={'agents':{'defaults':{'model':{'primary':'openai/unavailable'},'models':{'openai/unavailable':{'agentRuntime':{'id':'codex'}},'openai/available':{'agentRuntime':{'id':'codex'},'params':{'x':1}},'anthropic/other':{'agentRuntime':{'id':'embedded'}},'custom/provider-model':{'agentRuntime':{'id':'codex'}},'openai/internal':{'agentRuntime':{'id':'codex'}}}}}}
        cache={'fetched_at':'2026-11-19T00:00:00Z','models':[{'slug':'available','visibility':'list'},{'slug':'internal','visibility':'hide'}]}
        result=catalog.reconcile(config,cache,1795046400)
        models=result['agents']['defaults']['models']
        self.assertNotIn('openai/unavailable',models);self.assertIn('openai/internal',models)
        self.assertEqual(models['openai/available']['params'],{'x':1});self.assertIn('anthropic/other',models);self.assertIn('custom/provider-model',models)
        self.assertEqual(result['agents']['defaults']['model']['primary'],'openai/available')
        self.assertIn('openai/unavailable',config['agents']['defaults']['models'])
    def test_pruned_codex_model_is_removed_from_picker_policy(self):
        config={'agents':{'defaults':{'models':{'openai/unavailable':{'agentRuntime':{'id':'codex'}}},'modelPolicy':{'allow':['openai/unavailable','openai/available','anthropic/*'],'deny':['openai/blocked']}}}}
        cache={'fetched_at':'2026-11-19T00:00:00Z','models':[{'slug':'available','visibility':'list'}]}
        result=catalog.reconcile(config,cache,1795046400)
        self.assertEqual(result['agents']['defaults']['modelPolicy'],{'allow':['openai/available','anthropic/*'],'deny':['openai/blocked']})
    def test_policy_that_would_lose_all_explicit_choices_requires_review(self):
        config={'agents':{'defaults':{'models':{'openai/unavailable':{'agentRuntime':{'id':'codex'}}},'modelPolicy':{'allow':['openai/unavailable']}}}}
        cache={'fetched_at':'2026-11-19T00:00:00Z','models':[{'slug':'available','visibility':'list'}]}
        with self.assertRaises(ValueError):catalog.reconcile(config,cache,1795046400)
    def test_restricted_policy_requires_explicit_default_replacement(self):
        config={'agents':{'defaults':{'model':'openai/old','models':{'openai/old':{'agentRuntime':{'id':'codex'}}},'modelPolicy':{'allow':['openai/old','openai/b']}}}}
        cache={'fetched_at':'2026-11-19T00:00:00Z','models':[{'slug':'a','visibility':'list'},{'slug':'b','visibility':'list'}]}
        with self.assertRaises(ValueError):catalog.reconcile(config,cache,1795046400)
    def test_stale_or_empty_account_catalog_cannot_erase_configuration(self):
        for cache in [{'fetched_at':'2026-11-01T00:00:00Z','models':[{'slug':'available','visibility':'list'}]},{'fetched_at':'2026-11-19T00:00:00Z','models':[]}]:
            with self.assertRaises(ValueError):catalog.reconcile({},cache,1795046400)
    def test_available_new_model_is_added_with_codex_routing(self):
        cache={'fetched_at':'2026-11-19T00:00:00Z','models':[{'slug':'new-model','visibility':'list'}]}
        result=catalog.reconcile({},cache,1795046400)
        self.assertEqual(result.get('agents',{}).get('defaults',{}).get('models',{}).get('openai/new-model'),{'agentRuntime':{'id':'codex'}})
if __name__=='__main__':unittest.main()
