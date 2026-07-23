# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 1.x     | :white_check_mark: |
| < 1.0   | :x:                |

Only the latest `1.x` release is actively supported with security fixes.

## Reporting a Vulnerability

If you discover a security vulnerability in `easy_db_migrator_rust`, please report it privately rather than opening a public issue or pull request.

when reporting, please include:

- A description of the vulnerability and its potential impact
- Steps to reproduce, or a proof-of-concept if available
- The affected version(s) of the crate
- Any relevant configuration (database backend, connection setup) needed to reproduce it

## Scope

This policy covers the `easy_db_migrator_rust` crate itself. It does not cover:

- Vulnerabilities in third-party dependencies (`sqlx`, `tiberius`, `tokio`, etc.) — please report those to the respective upstream projects.
- Vulnerabilities in the PostgreSQL or SQL Server database servers this library connects to.
