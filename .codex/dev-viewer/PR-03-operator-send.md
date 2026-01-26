# PR-03: Add “Send Test Message” per platform preview using `greentic-operator send`

## Goal
Add a button on each dynamically discovered platform preview card to send the **exact shown preview output**
using `greentic-operator send`, so developers can validate conversion end-to-end immediately.

## Dependencies
- PR-01 merged (providers load)
- PR-02 merged (platforms are dynamic + conversion results exist)

## Scope
1. Add “Send Test” action per platform preview
2. Provide minimal destination/config inputs
3. Execute `greentic-operator send` safely (no shell injection)
4. Show send result inline (stdout/stderr + exit code)

---

## Implementation plan

### 1) Define send contract
Create module: `dev_viewer::operator_send`
- `OperatorSendRequest`
  - `provider_id`
  - `platform_id`
  - `payload` (string; the rendered platform output)
  - `destination` (string; user/channel/room id)
  - `extra_args` (optional)
- `OperatorSendResult`
  - `ok: bool`
  - `exit_code: i32`
  - `stdout: String`
  - `stderr: String`

### 2) Invocation strategy
Prefer **direct process execution**:
- `std::process::Command::new("greentic-operator")`
- `.arg("send") ...`
- No shell
- Timeout / cancellation support if GUI framework allows (otherwise a thread).

Add config knobs:
- `--operator-bin <path>` optional, default `greentic-operator` in PATH

### 3) UI additions
In each platform preview card:
- Add a “Send Test” button
- Add destination input:
  - simplest: a global destination field above previews
  - optionally override per card later
- Add “Dry run” toggle (prints command without executing)

Show results:
- Inline status: “Sent ✓” or “Failed ✗”
- Expandable logs (stdout/stderr)

### 4) Safety + DX
- Always log the effective command args (redact secrets if any).
- Validate destination non-empty before enabling send button.
- If operator binary missing: show a clear error with remediation.

---

## Files (suggested)
- `crates/dev-viewer/src/operator_send.rs` (new)
- `crates/dev-viewer/src/ui/platform_previews.rs`
  - add destination input + per-card send button
- `crates/dev-viewer/src/main.rs`
  - add `--operator-bin` flag plumbing

---

## Acceptance checks
- [ ] Each platform preview card has a “Send Test” button.
- [ ] Button sends the exact shown payload via `greentic-operator send`.
- [ ] Missing operator binary yields actionable error.
- [ ] stdout/stderr shown in UI after send.
- [ ] No panics/hangs when operator fails; errors are contained and visible.

## Suggested tests
- Unit test: command args building (no shell, deterministic).
- Integration test: fake operator binary script in temp dir that records args, verify payload/destination passed.

