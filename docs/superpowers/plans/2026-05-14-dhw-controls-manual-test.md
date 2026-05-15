# DHW Controls — Manual Integration Test Plan (Task 16)

**Branch:** `feature/dhw_controls` (17 commits ahead of `main`)
**Target:** ctc.lan production heater
**Deployed by:** Magnus
**Prerequisites:** release binary built (`cargo build --release -p server`); `config.toml` on ctc.lan has both `[homey] enabled = true` and `[smartgrid] enabled = true`; redb path / persistence path writable.

## Pre-flight (one-time)

1. Switch the heater from Manuell to Normal at the physical pump display (Menu → Varmvatten → Program → Normal). Verify:

```bash
curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61500&custom=true&factor=1.0"
# Expect: {"ctc_data": 1.0}
```

If this step is skipped, the DHW dropdown's comfort sub-menu will show "Manuell" until the user picks Eco/Normal/Komfort — acceptable but cosmetic.

## Six test cases (spec §6.4)

### 1. UC-A pre-flight hit (already-hot tank)

- Wait until `dhw_upper >= stop_temp`. Confirm:
  ```bash
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=62276&custom=true&factor=0.1"
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=62001&custom=true&factor=0.1"
  ```
- POST shower:
  ```bash
  curl -s -X POST "http://ctc.lan:3000/api/v1/dhw/boost?preset=shower"
  ```
- **Expected**: `200 OK` with body `{"outcome":"already_at_target","dhw_c":<>,"target_c":<>}`. No badge appears on the dashboard. `61503` register reads 0.

### 2. UC-A normal (cold tank)

- Wait for DHW upper to drop below stop temp (or wait for a draw to cool the tank). Confirm `dhw_upper < target_c`.
- POST shower. **Expected**: `200 OK` with `{"outcome":"started", "scheduled_end": "..."}`. Badge appears with "⚡ DHW Boost · 30 min". Verify:
  ```bash
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61503&custom=true&factor=0.5"  # Expect: 0.5
  ```
- 30 minutes later: badge disappears. No `61503=0` write (heater's own counter expired). Heating pump returns to SG-derived intent (whatever the current SG mode implies).

### 3. UC-B happy path (Bath, cheap window)

- During a cheap PriceLevel slot (chart shows "VeryCheap" or "Cheap" band on current hour).
- POST bath, 2 hours:
  ```bash
  curl -s -X POST "http://ctc.lan:3000/api/v1/dhw/boost?preset=bath&hours=2"
  ```
- **Expected**: `200 OK` with `{"outcome":"started", "scheduled_end": "..."}`. Badge shows "⚡ DHW Boost · 2 h 0 min". Verify:
  ```bash
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61503&custom=true&factor=0.5"  # Expect: 2.0
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61636&custom=true&factor=0.1"  # Expect: 50.0 (engage_temp)
  curl -s "http://ctc.lan:3000/api/v1/smartgrid"                              # Expect: mode=overcapacity
  ```
- The Homey heating-circ pump is off (override active).
- The spot-price chart shows a translucent warm-orange band over the next 2 h.
- If spot < 0.45 SEK/kWh: badge gains `· ⚙ immersion`; verify `61591 = 3.0`.

### 4. UC-B not-cheap reject

- During an expensive PriceLevel slot (`Normal`, `Expensive`, or `VeryExpensive`).
- POST bath, 1 hour. **Expected**: `409 Conflict` with body `{"error":"price_not_cheap","current_level":"Normal"}`. No state change, no Modbus writes.

### 5. UC-B early cancel

- During an active Bath (from case 3), click "Cancel boost" on the dashboard OR:
  ```bash
  curl -s -X DELETE "http://ctc.lan:3000/api/v1/dhw/boost"
  ```
- **Expected**: `204 No Content`. Badge disappears. Verify cleanup:
  ```bash
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61503&custom=true&factor=0.5"  # Expect: 0
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61591&custom=true&factor=0.1"  # Expect: 0.0
  curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61636&custom=true&factor=0.1"  # Expect: 60.0 (factory restore)
  curl -s "http://ctc.lan:3000/api/v1/smartgrid"                              # Expect: mode=normal
  ```

### 6. Crash recovery

- Start a Bath (case 3 setup).
- Kill the server: `kill -9 $(pgrep -f 'target/release/server')`
- Restart the server.
- **Expected**: log line "DHW recovery cleared mid-flight boost from previous run (...)". State is empty:
  ```bash
  curl -s "http://ctc.lan:3000/api/v1/dhw/state"  # Expect: boost: null
  ```
- All registers in baseline state (61503=0, 61591=0, 61636=60, SG=Normal). Heating pump on (reconciler converged to SG=Normal intent).

## UI smoke

- Open the dashboard. Confirm:
  - The DHW control button appears next to the SmartGrid control.
  - Comfort sub-menu shows the current level marked.
  - During an active boost, both Shower and Bath rows in the dropdown are disabled.
  - During an active Bath, a "Cancel boost" row appears.
  - The Bath confirm modal's slider snaps to 0.5/1.0/1.5/2.0 h.
  - The 5s polling refresh updates the badge in real time.

## Sign-off

When all six cases pass, append a date-stamped line to this file:

```
Manual integration test run on YYYY-MM-DDTHH:MMZ: all 6 cases pass. — Magnus
```

Then proceed to Task 17 (deploy / merge to main).
