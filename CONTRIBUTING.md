# Repository contribution and security policy

Use signed commits and pull requests for the default branch. Force-pushing or deleting the default branch is prohibited. Resolve review conversations before merging; prefer squash merges and delete merged topic branches. A second approval is not mandatory while there is only one maintainer, because GitHub does not allow authors to approve their own pull requests. CODEOWNERS identifies the maintainer but does not provide independent review by itself.

Run applicable local checks before pushing and verify the resulting CI run before merging. New scaffolds have no executable implementation or CI yet; add meaningful build, test and compatibility checks with the first code import, then make those checks required. A documentation scaffold is not evidence that migrated code has passed tests.

GitHub Actions must use read-only default token permissions and grant additional permissions only to the job that needs them. Actions must be approved and pinned to full commit SHAs. External fork workflows require maintainer approval; private forks must not receive secrets or write tokens. Do not use privileged pull_request_target or workflow_run jobs to check out and execute untrusted pull-request code. Do not expose persistent personal/self-hosted runners to untrusted contributions. Workflow approval is a security decision, not an automatic consequence of opening a pull request.

Do not commit tokens, SSH private keys, credentials, user terminal output, session recordings, uploads, local state or production configuration. Secret scanning and push protection are additional controls where supported, not a guarantee that all sensitive content is detected. Keep private and public repositories, build contexts, caches and artifacts separated. Review imports and their history before publishing; preserve original licenses and attribution.

Future release workflows must separate untrusted validation from credentialed publication, use minimal permissions, and publish only intentionally reviewed artifacts. Do not introduce cloud credentials, OIDC trust, package publication or deployments as part of scaffolding. Add ecosystem dependency updates, audits and code scanning when the corresponding code is imported; never add placeholder checks that claim nonexistent coverage.

See SECURITY.md for private vulnerability reporting. Repository settings are configured on GitHub and must be verified there; this policy file does not enforce them by itself.
