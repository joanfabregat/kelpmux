# Security policy

## Reporting a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/joanfabregat/kelpmux/security/advisories/new). Do not disclose suspected vulnerabilities, exploit details or credentials in public issues, discussions or pull requests. If private reporting is unavailable, contact the repository owner through an existing private channel to arrange a report; do not publish the details as a fallback.

Include affected versions or commits, prerequisites, impact and a minimal reproduction. Redact credentials and user data. Never attack a deployed instance, exhaust resources or access another user's workspace to demonstrate a problem without specific authorization.

## Supported code

Security reports about the default branch and latest published release are welcome. New scaffolds have no deployable release yet. Older releases do not have a promised long-term support window. The OSS server, client libraries and TUI share this workspace; KelpHub maintains its web and management code separately. Existing combined KelpMux releases remain supported during the packaging transition.

A terminal client can exercise the authenticated Linux user's authority. SSH authentication does not make executing untrusted terminal commands safe, and the web origin remains trusted frontend delivery code. Keep relay, user-owned services and credentials separated.
