# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.5.x   | Yes       |
| < 0.5   | No        |

## Reporting a Vulnerability

This project uses **GitHub Private Vulnerability Reporting**.

To report a security issue, go to the [Security tab](https://github.com/stasis-industries/mafis/security/advisories/new) of this repository and click **"Report a vulnerability"**.

You will receive a response within 7 days. Please **do not** open a public issue for security vulnerabilities.

## Scope

MAFIS is a research simulation tool that runs in the browser (WASM) and as a native desktop application. It does not handle authentication, user data, or network services. Security concerns relevant to this project:

- **Supply chain** — vulnerabilities in Rust dependencies (monitored via `cargo audit` in CI)
- **WASM sandbox** — theoretical escapes from the browser sandbox
- **Untrusted input** — malicious topology JSON files parsed by the simulator
