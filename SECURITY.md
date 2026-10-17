# Vulnerability reporting

Taskmaster's disclosure process is documented in two places:

- [Reporting policy](docs/src/security-policy.md) explains supported releases,
  what belongs in a report, and the expected response window.
- [Threat model](docs/src/security-model.md) describes Taskmaster's trust assumptions
  and the boundaries between the app, its Engine, child tools, remote hosts,
  and the updater.

## Contact

Send suspected security issues to
**[hi@nasrul.info](mailto:hi@nasrul.info)** and use `Taskmaster security` as the
message subject.

Share only the sensitive material needed to investigate the problem. In
particular, leave out credentials, exploit payloads, and private terminal
content whenever possible. If a report cannot be understood without that
material, clearly label the email confidential.
