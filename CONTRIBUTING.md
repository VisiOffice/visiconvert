# Contributing

Keep changes focused and describe the user-visible behavior and how you verified
it. For substantial features, open an issue to discuss the scope first.

## Local checks

Run installer checks with Python 3.11+ and the Docker Compose CLI:

```sh
python3 -m unittest discover -s tests -v
```

Build and exercise the full self-hosted application with Docker:

```sh
python3 scripts/verify-self-host.py
```

The integration runner creates uniquely named temporary installations and checks
clean startup, migrations, lead/booking persistence, backups, and restoration.
Its logs are written under `artifacts/`; do not commit those logs or private
configuration. Pass `--image <local-image-tag>` to reuse an already-built image.

The production Dockerfile runs Rust tests and compiles the web/WASM application.
The separate `verify` target runs browser/database verification:

```sh
docker build --target verify -t visiconvert-verify:local .
docker run --rm visiconvert-verify:local
```

Never include API keys, carrier credentials, database dumps, caller details, or
customer records in issues, fixtures, or commits. Use fictional data for tests.
Contributions must be work you have the right to submit under the repository's
license; retain notices for any third-party code.
