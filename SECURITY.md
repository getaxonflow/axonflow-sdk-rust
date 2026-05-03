# Security Policy

## Supported Versions

The Rust SDK has not been released yet — see [README.md](./README.md). Once a v0.1.0 is published, the most recent minor version will be the security-supported line. This file will be updated to track the supported range at that point.

## Reporting a Vulnerability

We take security seriously at AxonFlow. If you discover a vulnerability — in this SDK, in another AxonFlow SDK, or in the AxonFlow control plane — please follow responsible disclosure:

### Do NOT
- Open a public GitHub issue
- Discuss the vulnerability publicly
- Exploit the vulnerability

### DO
1. Email: **security@getaxonflow.com**
2. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if any)

### What to expect
- **24 hours**: Initial response acknowledging receipt
- **72 hours**: Assessment and severity classification
- **7 days**: Fix timeline and coordinated disclosure plan
- **30 days**: Public disclosure after fix is released

### Severity levels
- **Critical**: Remote code execution, authentication bypass
- **High**: Data leakage, privilege escalation
- **Medium**: Denial of service, information disclosure
- **Low**: Minor issues with limited impact

## Security Best Practices for SDK Users

When the SDK lands, expect the same patterns as the [TypeScript](https://github.com/getaxonflow/axonflow-sdk-typescript), [Python](https://github.com/getaxonflow/axonflow-sdk-python), [Go](https://github.com/getaxonflow/axonflow-sdk-go), and [Java](https://github.com/getaxonflow/axonflow-sdk-java) SDKs:

1. **Never hardcode API keys** — read from environment variables (`AXONFLOW_API_KEY`).
2. **Rotate API keys** quarterly.
3. **Monitor audit logs** for unusual activity.
4. **Keep the SDK updated** to the latest minor version.

### Example secure usage (illustrative; SDK not yet implemented)

```rust
// Read from environment, do not commit secrets
let api_key = std::env::var("AXONFLOW_API_KEY")
    .expect("AXONFLOW_API_KEY must be set");

let client = axonflow::Client::new(&api_key);
```

## Supply Chain

Once published to crates.io, expect:

- Releases will be cargo-published from a tagged commit only (no manual `cargo publish` from a developer machine)
- All commits on the release tag signed
- Branch protection on `main` requires green CI before merge
- Dependabot enabled for transitive crates

## Vulnerability Disclosure Timeline

We follow a 90-day disclosure timeline:

1. **Day 0**: Vulnerability reported
2. **Day 7**: Fix developed and tested
3. **Day 14**: Fix released in patch version
4. **Day 30**: Public disclosure (if fix is deployed)
5. **Day 90**: Full technical details published (if not disclosed earlier)

## Hall of Fame

We recognize security researchers who responsibly disclose vulnerabilities.

(No vulnerabilities reported yet — be the first!)

## Contact

- **Security issues**: security@getaxonflow.com
- **General support**: hello@getaxonflow.com
- **GitHub Security Advisories**: https://github.com/getaxonflow/axonflow-sdk-rust/security/advisories

Thank you for keeping AxonFlow secure.
