# Security policy

Autophagy handles coding-agent transcripts, commands, file paths, diffs, and
other sensitive local data. Please do not open public issues for vulnerabilities
or accidentally include private session data in reports.

Report security issues privately through GitHub Security Advisories for this
repository. Include a minimal, redacted reproduction and the affected version.

## Security posture

- No telemetry or cloud processing by default.
- Explicit project inclusion, exclusions, and retention controls.
- Redaction before persistence or any user-authorized cloud boundary.
- Generated mutations are inspectable, permission-scoped, and reversible.
- Generated scripts never become executable without explicit approval.

The project is pre-alpha and does not yet make production security guarantees.
