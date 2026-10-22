# Connect Twilio in either edition

The Rust engine and Vite application provide the same Twilio connection flow
for self-hosted and hosted workspaces. Rails handles hosted identity and billing;
it does not store the carrier Auth Token or participate in call routing.

This milestone verifies account access and number ownership. It does not buy
numbers or change Twilio configuration. Live forwarding and independent fallback
still require a real test call before production traffic is enabled.

## Prepare the server

Set `PUBLIC_URL` to the exact HTTPS origin serving the engine. Configure a stable
`VISICONVERT_ENCRYPTION_KEY`: 32 random bytes encoded as 64 hexadecimal characters,
independent of the workspace login key. Generate it privately with a cryptographic
random generator. Keep the key with your encrypted backups; a database dump alone
cannot recover saved carrier credentials. Changing this key is not a supported
rotation procedure and makes existing credentials unreadable.

New self-host installations generate the encryption key automatically. For an
existing installation, preserve its configuration and run:

```sh
bash scripts/self-host.sh enable-connections
bash scripts/self-host.sh up
```

Use the normal `--directory` option if your configuration lives elsewhere.
The first command adds a missing key without replacing database or login keys.
The second applies migrations and restarts the app. Set up HTTPS separately.
Development installations created before this change also need the key added to
their private `.env`. Hosted operators supply it to the engine environment.

## Connect and register a number

1. Open **Connections** as the workspace operator or hosted workspace owner.
2. Enter the Twilio Account SID and that account's Auth Token, then choose
   **Verify and connect Twilio**. For a subaccount, use its own SID and token.
3. Choose **Check available tracking numbers**. The inventory lists up to 100
   supported numbers; you can also enter another owned number directly.
4. In **Tracking numbers**, register an existing voice-capable +1 number, its
   marketing source/campaign, and the business phone that should receive calls.
   Account ownership and voice capability are checked against Twilio on save.
5. Copy the workspace's incoming voice and status callback URLs from
   **Connections** into the Twilio number configuration, both with HTTP POST.
   The engine supplies the scoped dial-completion URL in its returned TwiML.
6. Place a test call. Verify the destination rings, source attribution appears,
   and the final call outcome is recorded. Independently configure and test
   Twilio's fallback with the engine/database unavailable.

The adapter currently uses Twilio's default US1 REST API. It accepts +1 phone
number syntax; it does not establish country eligibility, destination dialing
permissions, or trial-account restrictions. Destination ownership is not checked.
Twilio charges still apply. Recording, SMS, number purchasing, dynamic number
pools, and automated provider configuration are outside this milestone.

Account verification uses a read-only account request. Number lookup uses the
read-only incoming-number API. Provider response bodies and tokens are never
returned to the browser. Tokens are encrypted with AES-256-GCM and bound to both
the workspace and account. Public callbacks verify signatures with that
workspace's token and reject wrong account IDs, unknown numbers, and mismatched
callback URLs. Billing is outside the callback path.

## Updates, disconnects, and existing installations

To update a token, submit the same Account SID and its current Auth Token in
Connections. Keep the prior token valid until the replacement is saved where
your provider rotation procedure permits it. There is no dual-token acceptance
window. A stale page receives a conflict instead of overwriting newer changes.

An account cannot be changed or disconnected while the workspace has registered
non-demo tracking numbers. Number removal and account migration require a future
controlled workflow; they are not available in this UI yet.

Existing `TWILIO_ACCOUNT_SID` / `TWILIO_AUTH_TOKEN` environment configurations
continue to use the original `/webhooks/twilio/{endpoint}` callbacks for the
default operator workspace. They cannot be overwritten from the UI. Migrating
them is an explicit cutover: configure an independent fallback, remove the
legacy credentials from the app environment, restart, save the same account in
Connections, and replace the provider callbacks with the scoped URLs. Expect an
interruption between removing legacy credentials and completing the cutover.
Do not start the app with both a saved connection and legacy environment
credentials for that default workspace.

Self-hosted installations without a provider connection may still save offline
number configurations for evaluation; those numbers do not route calls. Hosted
workspaces must connect Twilio before registering numbers.

## API and verification

`GET /api/connections/twilio` is the authoritative connection status and callback
URL endpoint. `PUT` accepts `account_sid`, `auth_token`, and the current `revision`
(0 when disconnected). `DELETE` accepts the current `revision` and requires no
active numbers. `GET /api/connections/twilio/numbers` returns owned voice numbers.
All require an authenticated operator or hosted owner. The older bootstrap
provider fields describe only legacy environment configuration.

The database integration suite uses a local simulated Twilio API and tests both
authentication modes, ownership checks, credential encryption and reloading,
conflicting changes, disconnect/reconnect revisions, tenant isolation, signed
callbacks, and duplicate delivery. Run it against a fresh disposable database:

```sh
VISICONVERT_TEST_DATABASE_URL='postgresql://localhost/visiconvert_test' \
  cargo test -p convert-server connection_tests::shared_connections -- --ignored
```

This test keeps fixtures in its disposable database. Recreate that database
between runs. `scripts/check.sh` and the isolated browser verification harness
also include this test. An opt-in `connection_tests::manual_browser_fixture`
serves the production frontend on loopback port 43113 with simulated provider
credentials when `VISICONVERT_BROWSER_FIXTURE_DIR` and the disposable database
URL are supplied. It is compiled only in tests and stops with Ctrl-C.

References: [Twilio request validation](https://www.twilio.com/docs/usage/security),
[Twilio account authentication](https://www.twilio.com/docs/usage/requests-to-twilio),
[incoming number API](https://www.twilio.com/docs/phone-numbers/api/incomingphonenumber-resource).
