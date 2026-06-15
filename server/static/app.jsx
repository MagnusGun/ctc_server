const { useState, useEffect, useMemo } = React;

/* ---------- Tweak defaults (persisted via host) ---------- */
const TWEAK_DEFAULTS = /*EDITMODE-BEGIN*/{
  "theme": "dark",
  "accent": "mint",
  "density": "comfortable",
  "cards": "outlined",
  "showSparklines": true
}/*EDITMODE-END*/;

const ACCENTS = {
  mint:   { c: "oklch(0.78 0.14 165)" },
  ice:    { c: "oklch(0.78 0.13 235)" },
  amber:  { c: "oklch(0.82 0.14 75)"  },
  coral:  { c: "oklch(0.78 0.14 25)"  },
};

// Trend modal configs. Each key maps to one or more sensors fetched on demand
// via /api/v1/heatpump/series. `data` arrays are filled in `useEffect` after
// fetch — placeholder zeros keep the chart from crashing during the loading
// window (<1 s).
const TREND_CONFIGS = {
  room:     { title: "Room temperature", sub: "Living room sensor", unit: "°C", yMin: 18, yMax: 24,
              sensors: [{ slug: "room", label: "Room", color: "oklch(0.85 0.13 60)" }] },
  outdoor:  { title: "Outdoor temperature", sub: "External sensor", unit: "°C", yMin: 8, yMax: 20,
              sensors: [{ slug: "outdoor", label: "Outdoor", color: "var(--cold)" }] },
  dhw:      { title: "Hot water tank", sub: "Upper & lower sensors", unit: "°C", yMin: 38, yMax: 56,
              sensors: [
                { slug: "dhw_upper", label: "Upper (DHW)", color: "var(--hot)" },
                { slug: "lower",     label: "Lower (RAD)", color: "oklch(0.85 0.13 60)" },
              ] },
  flow:     { title: "Heating system · Flow / Return", sub: "Supply, return and outdoor", unit: "°C", yMin: 10, yMax: 36,
              sensors: [
                { slug: "flow",    label: "Flow",    color: "var(--hot)"    },
                { slug: "return",  label: "Return",  color: "var(--accent)" },
                { slug: "outdoor", label: "Outdoor", color: "var(--cold)"   },
              ] },
  hp:       { title: "Heat pump · In / Out", sub: "Refrigerant-side temperatures", unit: "°C", yMin: 18, yMax: 32,
              sensors: [
                { slug: "hp_in",  label: "HP In",  color: "oklch(0.85 0.13 60)" },
                { slug: "hp_out", label: "HP Out", color: "var(--hot)" },
              ] },
  gas:      { title: "Compressor · Discharge / Suction", sub: "Hot gas vs cold gas", unit: "°C", yMin: 18, yMax: 30,
              sensors: [
                { slug: "discharge", label: "Discharge", color: "var(--hot)"  },
                { slug: "suction",   label: "Suction",   color: "var(--cold)" },
              ] },
  pressure: { title: "Refrigerant pressures", sub: "High side vs low side", unit: "bar", yMin: 5, yMax: 10,
              sensors: [
                { slug: "high_p", label: "High P", color: "var(--hot)"  },
                { slug: "low_p",  label: "Low P",  color: "var(--cold)" },
              ] },
  brine:    { title: "Brine loop · In → Out", sub: "Collector loop temperatures", unit: "°C", yMin: 12, yMax: 20,
              sensors: [
                { slug: "brine_in",  label: "Brine In",  color: "var(--cold)"   },
                { slug: "brine_out", label: "Brine Out", color: "var(--accent)" },
              ] },
};

const STATS_TAB_TITLES = {
  cycles:  "Cycle times",
  starts:  "Compressor starts",
  hours:   "Operating hours",
  heating: "Heating system",
};

// HEATING_STATUS register (62246) — operating regime, not live heat delivery.
// 0 Off (grey) · 1 Vacation, 2 Night (amber — limited operation) · 3 On (green — regular regime, heat-curve in effect).
const HEATING_STATUS_CLASS = { 0: "off", 1: "warn", 2: "warn", 3: "on" };
// Derive a human-readable explanation of the current heating state.
// Cross-references status (62246) with outdoor temp and the heat-off threshold
// so the user sees *why* heating is currently in this state, not just the label.
const whyHeating = (status, outdoor, heatOff) => {
  const haveBoth = outdoor != null && heatOff != null;
  switch (status) {
    case 0:
      return (haveBoth && outdoor > heatOff)
        ? `Off · Outdoor ${outdoor.toFixed(1)}°C > heat-off threshold ${heatOff.toFixed(1)}°C`
        : "Off · heat curve disabled (manual or hysteresis pending)";
    case 1:
      return "Vacation reduction active";
    case 2:
      return "Night-time reduction active";
    case 3:
      if (haveBoth && outdoor >= heatOff) {
        return `On · heat-off pending (outdoor ${outdoor.toFixed(1)}°C ≥ threshold ${heatOff.toFixed(1)}°C, timer not yet elapsed)`;
      }
      return heatOff != null
        ? `On · outdoor below heat-off threshold (${heatOff.toFixed(1)}°C)`
        : "On · heat curve in effect";
    default:
      return "Status · Off / Vacation / Night / On";
  }
};
// Mode label (Auto/On/Off) → tone. Auto and On both indicate the controller
// is actively driving the heating circuit; Off mutes it.
const HEATING_MODE_CLASS = { "Auto": "on", "On": "on", "Off": "off" };

const PRICE_LEVEL_LABELS = {
  very_cheap:     "Very Cheap",
  cheap:          "Cheap",
  normal:         "Normal",
  expensive:      "Expensive",
  very_expensive: "Very Expensive",
};
window.PRICE_LEVEL_LABELS = PRICE_LEVEL_LABELS;

// Backend SmartGrid mode strings ↔ UI keys. The UI uses "powersave" where the
// backend says "blocking"; the other three names match.
const BACKEND_TO_UI_MODE = {
  normal:       "normal",
  blocking:     "powersave",
  lowprice:     "lowprice",
  overcapacity: "overcapacity",
};
const UI_TO_BACKEND_MODE = Object.fromEntries(
  Object.entries(BACKEND_TO_UI_MODE).map(([backend, ui]) => [ui, backend]),
);

/* ---------- Spot-price helpers ----------
 * The backend serves PricePoint[] at 15-minute resolution (96 slots/day since
 * elprisetjustnu.se switched to quarter-hourly data in Oct 2025).
 */
const findPriceSlotAt = (prices, when) => {
  if (!Array.isArray(prices) || prices.length === 0) return -1;
  const t = when.getTime();
  for (let i = 0; i < prices.length; i++) {
    const s = Date.parse(prices[i]?.starts_at);
    const e = Date.parse(prices[i]?.ends_at);
    if (!Number.isNaN(s) && !Number.isNaN(e) && t >= s && t < e) return i;
  }
  return -1;
};

const formatHM = (iso) => {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return "—";
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
};
window.formatHM = formatHM;

/* ---------- DHW control ---------- */

// Comfort-level labels for the trigger button.
const COMFORT_LEVEL_LABELS = {
  economy: "Economy",
  normal:  "Normal",
  komfort: "Comfort",
  manuell: "Manual",
};

// Writable comfort levels (Manuell is read-only; the heater derives it from
// 61500=3 with a custom stop-temp). UI surfaces only the three the backend
// route accepts.
const COMFORT_OPTIONS = [
  { value: "economy", label: "Economy" },
  { value: "normal",  label: "Normal"  },
  { value: "komfort", label: "Comfort" },
];

// Format a remaining-seconds value as either "{m} min" (≤60min) or
// "{h} h {m} min". Always rounds up to the next minute so the user sees
// "1 min" until the boost is fully done rather than "0 min" mid-final-tick.
const formatBoostRemaining = (remainingS) => {
  const remMin = Math.ceil(Math.max(0, remainingS ?? 0) / 60);
  if (remMin < 60) return `${remMin} min`;
  const h = Math.floor(remMin / 60);
  const m = remMin % 60;
  return `${h} h ${m} min`;
};

/* DhwControl — closed-state trigger + dropdown menu with three rows:
 *  • Shower (30 min)
 *  • Bath (custom)  — opens a modal owned by the parent via `onOpenBath`
 *  • Comfort level submenu
 *
 * Active-boost handling:
 *  • Shower and Bath rows disable while a boost is active.
 *  • A "Cancel boost" row appears only for an active Bath (Shower has no
 *    cancel — the backend rejects it with 409 shower_runs_to_completion).
 *  • A transient toast string is returned via `onToast(text)` for
 *    `started: false` / 409 paths.
 *
 * The component is parent-controlled for the bath modal so the modal can
 * render at app-root z-index and the component can stay focused on the
 * dropdown affordance.
 */
function DhwControl({ dhwResp, sysStatus, onRefetch, onOpenBath, onToast }) {
  const [open, setOpen] = React.useState(false);
  const [submenuOpen, setSubmenuOpen] = React.useState(false);

  const boost = dhwResp?.boost;
  const comfortLevel = dhwResp?.comfort_level ?? "normal";
  const comfortLabel = COMFORT_LEVEL_LABELS[comfortLevel] ?? comfortLevel;
  const presetKind = boost?.preset?.kind;
  const bathActive = presetKind === "bath";
  const showerActive = presetKind === "shower";
  const boostActive = !!boost;
  // Shower is also implicitly "active" when the heater is already in DHW
  // (Sensor::SystemStatus = 5) — pressing Shower would return
  // {started:false, already_at_target} so we disable it as a courtesy hint.
  const dhwCharging = sysStatus === 5;

  const triggerLabel = boostActive
    ? (showerActive
        ? `⚡ Shower · ${formatBoostRemaining(boost.remaining_s)}`
        : `⚡ Bath · ${formatBoostRemaining(boost.remaining_s)}`)
    : comfortLabel;

  const close = () => { setOpen(false); setSubmenuOpen(false); };

  const handleShower = async () => {
    close();
    try {
      const r = await window.api.startDhwBoost("shower");
      if (r?.outcome === "already_at_target") {
        onToast?.(`Already at target (${r.dhw_c?.toFixed?.(1) ?? "—"} ≥ ${r.target_c?.toFixed?.(1) ?? "—"} °C)`);
      }
    } catch (e) {
      onToast?.(e?.body?.error || e?.message || "Failed to start shower boost");
    } finally {
      onRefetch?.();
    }
  };

  const handleComfort = async (level) => {
    close();
    try {
      await window.api.setDhwComfort(level);
    } catch (e) {
      onToast?.(e?.message || `Failed to set comfort: ${level}`);
    } finally {
      onRefetch?.();
    }
  };

  const handleCancelBath = async () => {
    close();
    try {
      await window.api.cancelDhwBoost();
    } catch (e) {
      onToast?.(e?.body?.error || e?.message || "Failed to cancel bath boost");
    } finally {
      onRefetch?.();
    }
  };

  return (
    <div className="dhw-wrap">
      <button className={`chip dhw-trigger ${boostActive ? "active" : ""}`}
              onClick={() => setOpen(o => !o)}
              aria-expanded={open}
              title={boostActive ? "Active DHW boost" : "Hot water control"}>
        <span className="label">Hot Water</span>
        <span className="value">{triggerLabel}</span>
        <svg width="10" height="10" viewBox="0 0 12 12" fill="none"
             stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"
             style={{ marginLeft: 6, opacity: 0.6, transform: open ? "rotate(180deg)" : "" }}>
          <path d="M3 4.5l3 3 3-3"/>
        </svg>
      </button>
      {open && (
        <>
          <div className="sg-back" onClick={() => close()}/>
          <div className="sg-pop dhw-pop" onClick={e => e.stopPropagation()}>
            <div className="sg-pop-head">Hot water</div>

            <button className="sg-opt"
                    disabled={boostActive || dhwCharging}
                    onClick={handleShower}>
              <span className="sg-dot"/>
              <span className="sg-text">
                <span className="t">⚡ Shower (30 min)</span>
                <span className="d">Boost DHW to target for 30 minutes.</span>
              </span>
            </button>

            <button className="sg-opt"
                    disabled={boostActive}
                    onClick={() => { close(); onOpenBath?.(); }}>
              <span className="sg-dot"/>
              <span className="sg-text">
                <span className="t">⚡ Bath (custom hours)</span>
                <span className="d">Plan a longer DHW boost with immersion safety gate.</span>
              </span>
            </button>

            <button className="sg-opt"
                    onClick={() => setSubmenuOpen(o => !o)}
                    aria-expanded={submenuOpen}>
              <span className="sg-dot"/>
              <span className="sg-text">
                <span className="t">🌡 Comfort level ▸</span>
                <span className="d">Active: {comfortLabel}</span>
              </span>
            </button>

            {submenuOpen && (
              <div className="dhw-submenu">
                {COMFORT_OPTIONS.map(o => (
                  <button key={o.value}
                          className={`sg-opt ${comfortLevel === o.value ? "active" : ""}`}
                          onClick={() => handleComfort(o.value)}>
                    <span className="sg-dot"/>
                    <span className="sg-text">
                      <span className="t">{o.label}</span>
                    </span>
                    {comfortLevel === o.value && (
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                           stroke="currentColor" strokeWidth="2.5"
                           strokeLinecap="round" strokeLinejoin="round">
                        <path d="M5 12l5 5L20 7"/>
                      </svg>
                    )}
                  </button>
                ))}
              </div>
            )}

            {bathActive && (
              <button className="sg-exit"
                      onClick={handleCancelBath}>
                Cancel boost
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/* DhwBathModal — confirm-and-apply dialog for the Bath preset.
 *
 * Slider range: [0.5, max] in 0.5 steps. `max` comes from `dhwResp.bath_max_hours`
 * when present (future snapshot extension), else falls back to 2.0 — the
 * server's current default (`DhwConfig::default::bath_max_hours = 2.0`).
 *
 * Error handling: 409 price_not_cheap and 422 out_of_range keep the modal
 * open with an inline error; the user can adjust hours and retry.
 *
 * TODO: surface the immersion-gate price ceiling
 * (`cfg.immersion_allow_price_sek_per_kwh`) from the snapshot or a future
 * config endpoint instead of hardcoding 0.50.
 */
function DhwBathModal({ open, dhwResp, onClose, onApplied }) {
  const [hours, setHours] = React.useState(1.0);
  const [busy, setBusy] = React.useState(false);
  const [inlineError, setInlineError] = React.useState(null);
  const maxHours = dhwResp?.bath_max_hours ?? 2.0;
  const minHours = 0.5;
  const step = 0.5;
  const priceCeiling = dhwResp?.immersion_allow_price_sek_per_kwh ?? 0.50;

  // Reset slider + clear error every time the modal opens so a previous
  // failed attempt's state doesn't bleed into the next open.
  React.useEffect(() => {
    if (!open) return;
    setHours(1.0);
    setInlineError(null);
    setBusy(false);
  }, [open]);

  if (!open) return null;

  const apply = async () => {
    setBusy(true);
    setInlineError(null);
    try {
      await window.api.startDhwBoost("bath", hours);
      onApplied?.();
      onClose?.();
    } catch (e) {
      const body = e?.body || {};
      if (e?.status === 409 && body.error === "price_not_cheap") {
        const lvl = body.current_level || "current";
        setInlineError(`Price is currently not cheap (${lvl}).`);
      } else if (e?.status === 422 && body.error === "out_of_range") {
        const min = body.min ?? minHours;
        const max = body.max ?? maxHours;
        setInlineError(`Hours out of range (${min}–${max}).`);
      } else {
        setInlineError(body.error || e?.message || "Failed to start bath boost.");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="scrim" onClick={onClose}>
      <div className="dialog dhw-bath-dialog" onClick={e => e.stopPropagation()}>
        <div className="head">
          <div className="glyph warm"><Icon name="bolt" size={16}/></div>
          <h3>Plan a bath boost</h3>
        </div>
        <div className="body">
          <p style={{ margin: "0 0 12px 0" }}>
            Boost the hot-water tank for a custom duration. The immersion
            heater will engage only while spot price is below the gate.
          </p>
          <div className="dhw-slider-row">
            <label htmlFor="dhw-bath-hours" className="dhw-slider-label">
              Duration <strong>{hours.toFixed(1)} h</strong>
            </label>
            <input id="dhw-bath-hours"
                   type="range"
                   min={minHours}
                   max={maxHours}
                   step={step}
                   value={hours}
                   onChange={e => setHours(Number(e.target.value))}/>
            <div className="dhw-slider-ticks">
              <span>{minHours.toFixed(1)} h</span>
              <span>{maxHours.toFixed(1)} h</span>
            </div>
          </div>
          <div className="dhw-gate-note">
            Immersion gate: ≤ {priceCeiling.toFixed(2)} SEK/kWh
          </div>
          {inlineError && (
            <div className="dhw-bath-error">{inlineError}</div>
          )}
        </div>
        <div className="actions">
          <button className="btn" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn primary warm"
                  onClick={apply}
                  disabled={busy}>
            {busy ? "Applying…" : `Apply ${hours.toFixed(1)} h`}
          </button>
        </div>
      </div>
    </div>
  );
}

/* ---------- App ---------- */

function App() {
  const [t, setTweak] = useTweaks(TWEAK_DEFAULTS);
  const [now, setNow] = useState(new Date());
  const [mode, setMode] = useState("normal");
  const [confirm, setConfirm] = useState(null);
  const [resumeMode, setResumeMode] = useState("schedule");
  const [resumeCandidates, setResumeCandidates] = useState([]);
  const [pickedResume, setPickedResume] = useState(0); // index into candidates
  const [trendKey, setTrendKey] = useState(null);
  const [trendData, setTrendData] = useState(null); // [{label, data, color}]
  const [trendError, setTrendError] = useState(null);
  const [statsTab, setStatsTab] = useState(null); // null | 'cycles' | 'hours'
  const [proposedResume, setProposedResume] = useState(null);
  const [version, setVersion] = useState(null);

  const powersave = mode === "powersave";
  const overcap   = mode === "overcapacity";
  const lowprice  = mode === "lowprice";
  const [sgOpen, setSgOpen] = useState(false);
  const narrow = useIsNarrow();

  // Per-device collapse state. Defaults: charts open on desktop, closed on
  // phone (`narrow` snapshot used only at first mount; the user's localStorage
  // override sticks across reloads). Cards default open everywhere.
  const [chartCollapse, setChartCollapse] = window.useCollapseState("ctc.collapsibles", {
    energyChart: !narrow,
    heatingTrend: !narrow,
    activityTimeline: !narrow,
  });
  const [cardCollapse, setCardCollapse] = window.useCollapseState("ctc.cards", {
    prices:        false,
    heatingSystem: false,
    heatPump:      false,
    stats:         true,
    energy:        false,
    activity:      false,
    messages:      false,
  });
  // Per-device "hide entire card" visibility. Separate from `cardCollapse`
  // (which just hides the body) — invisible cards aren't rendered at all.
  // Persisted in localStorage so the choice survives reloads without an
  // editor host (tweak EDITMODE persistence only fires when the host is up).
  const [cardVis, setCardVis] = window.useCollapseState("ctc.cardVisibility", {
    prices:        true,
    heatingSystem: true,
    heatPump:      true,
    stats:         true,
    energy:        true,
    activity:      true,
    messages:      true,
  });

  /* ---------- Live data from backend ---------- */
  const [activitySegs] = useActivity();
  const [temps, tempsMeta] = useTemperatures();
  const [heating]      = useHeatingSystem();
  const [hp]           = useHeatPump();
  const [hpStats]      = useHeatPumpStats();
  const [hpHistory]    = useHeatPumpHistory(365);
  const [hpFlow]       = useSeries("flow", 24);
  const [hpReturn]     = useSeries("return", 24);
  const [hpOutdoor]    = useSeries("outdoor", 24);
  const [roomSeries]   = useSeries("room", 24);
  const [dhwUpperSeries] = useSeries("dhw_upper", 24);
  const [lowerSeries]  = useSeries("lower", 24);
  const [alarmsResp]   = useAlarms();
  const [sgResp, sgMeta] = useSmartGrid();
  const [pumpResp, pumpMeta] = usePump();
  const [dhwResp, dhwMeta] = useDhwState();
  const [dhwToast, setDhwToast] = useState(null);
  const [bathModalOpen, setBathModalOpen] = useState(false);
  // Auto-clear toast after 4s.
  useEffect(() => {
    if (!dhwToast) return undefined;
    const id = setTimeout(() => setDhwToast(null), 4000);
    return () => clearTimeout(id);
  }, [dhwToast]);
  const [stepEvents]   = useStepResponse(6);
  const stepData = useMemo(() => window.transformStepEvents(stepEvents), [stepEvents]);

  // Build 24h sparkline arrays from the /heatpump/series fetches (already
  // bucketed to 1-minute means server-side). Null until the series lands;
  // bucketMinutely leaves null gaps for empty minutes.
  const sparks = useMemo(() => ({
    room:   roomSeries     ? window.bucketMinutely(roomSeries)     : null,
    out:    hpOutdoor      ? window.bucketMinutely(hpOutdoor)      : null,
    upper:  dhwUpperSeries ? window.bucketMinutely(dhwUpperSeries) : null,
    lower:  lowerSeries    ? window.bucketMinutely(lowerSeries)    : null,
    flow:   hpFlow         ? window.bucketMinutely(hpFlow)         : null,
    ret:    hpReturn       ? window.bucketMinutely(hpReturn)       : null,
  }), [roomSeries, hpOutdoor, dhwUpperSeries, lowerSeries, hpFlow, hpReturn]);

  // Stats data shapes derived from history + series fetches. `null` until
  // history arrives; the chart components fall back to their synth defaults
  // when given `undefined`.
  const statsData = useMemo(
    () => window.buildStatsData(hpHistory, hpFlow, hpReturn),
    [hpHistory, hpFlow, hpReturn]
  );
  const [pricesResp]   = usePrices();
  const [gridResp]     = useGrid();

  // Formatters: tolerate null/undefined values gracefully.
  const f1 = v => (v == null || Number.isNaN(v) ? "—" : Number(v).toFixed(1));
  const f2 = v => (v == null || Number.isNaN(v) ? "—" : Number(v).toFixed(2));
  const fInt = v => (v == null ? "—" : String(Math.trunc(v)));

  // Sync local SmartGrid mode with backend reading. Functional setMode reads
  // the latest mode without taking it as a dep, which avoids stale closures.
  useEffect(() => {
    if (!sgResp?.smartgrid_mode) return;
    const mapped = BACKEND_TO_UI_MODE[String(sgResp.smartgrid_mode).toLowerCase()];
    if (mapped) setMode(cur => (cur === mapped ? cur : mapped));
  }, [sgResp?.smartgrid_mode]);

  const MODE_META = {
    normal:       { label: "Normal",       desc: "System runs as scheduled.",                          tone: ""     },
    powersave:    { label: "Block",        desc: "Block heating during expensive hours.",              tone: ""     },
    lowprice:     { label: "Low price",    desc: "Pre-heat the buffer when electricity is cheap.",     tone: "cool" },
    overcapacity: { label: "Overcapacity", desc: "Boost — heat aggressively while there is surplus.",  tone: "warm" },
  };

  // Live ticking clock
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(id);
  }, []);

  // Fetch server version once on mount.
  useEffect(() => {
    window.api.getVersion()
      .then(r => setVersion(r))
      .catch(() => {});
  }, []);

  // Fetch the server's proposed auto-resume slot whenever the user opens the
  // "enter mode" confirm dialog. Used to render real cheapest-slot values
  // (replaces the previous hardcoded "23:30 / 0.14").
  useEffect(() => {
    if (confirm?.kind !== "enter") return;
    let aborted = false;
    window.api.getSmartGridResume()
      .then(r => { if (!aborted) setProposedResume(r); })
      .catch(() => {});
    if (confirm.target === "powersave") {
      window.api.getResumeCandidates()
        .then(r => {
          if (aborted) return;
          const cands = r?.candidates || [];
          setResumeCandidates(cands);
          setPickedResume(0);
          setResumeMode(cands.length ? "schedule" : "manual");
        })
        .catch(() => {
          if (aborted) return;
          setResumeCandidates([]);
          setResumeMode("manual");
        });
    }
    return () => { aborted = true; };
  }, [confirm]);

  // Apply theme & density to root
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", t.theme);
    document.documentElement.setAttribute("data-density", t.density);
    document.documentElement.setAttribute("data-cards", t.cards);
    document.documentElement.style.setProperty("--accent", ACCENTS[t.accent].c);
    document.documentElement.style.setProperty(
      "--accent-2",
      `color-mix(in oklch, ${ACCENTS[t.accent].c} 18%, transparent)`
    );
  }, [t.theme, t.density, t.cards, t.accent]);

  const clockStr = now.toLocaleTimeString("sv-SE", { hour12: false });
  const clockHM  = now.toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false });
  const dateStr = now.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });

  // Now index for chart — fractional 0..96 (one unit = 15 min).
  const nowIdx = (now.getHours() * 60 + now.getMinutes()) / 15;

  // Price-chart data — hoisted to App scope so both the Heating System card
  // (where the chart now lives) and the Energy & Prices card (live price box)
  // can read from a single source of truth.
  const todayPrices = pricesResp?.today?.prices || [];
  const nowSlotIdx  = findPriceSlotAt(todayPrices, now);
  const nowSlot     = nowSlotIdx >= 0 ? todayPrices[nowSlotIdx] : null;
  const nowPrice    = pricesResp?.current?.spot_sek ?? nowSlot?.spot_sek ?? null;
  const nowLevel    = pricesResp?.current?.level ?? nowSlot?.level ?? null;

  // System messages — fed by /api/v1/alarms.
  // Backend shape: { alarms: AlarmMessage[], infos: AlarmMessage[] }.
  // AlarmMessage = { reference, code, message, message_en, description, description_sv, first_seen }.
  const messages = useMemo(() => {
    if (!alarmsResp) return [];
    const out = [];
    const stamp = ts => (typeof ts === "string" ? ts.slice(11, 19) : "");
    const codeOrRef = (a, prefix) => a.code || (a.reference != null ? `${prefix}${a.reference}` : prefix);
    (alarmsResp.alarms || []).forEach(a => {
      out.push({
        tone: "error",
        code: codeOrRef(a, "E"),
        title: a.message_en || a.message || "Alarm",
        desc: a.description || a.description_sv || "",
        when: stamp(a.first_seen),
      });
    });
    (alarmsResp.infos || []).forEach(m => {
      out.push({
        tone: "info",
        code: codeOrRef(m, "I"),
        title: m.message_en || m.message || "Info",
        desc: m.description || m.description_sv || "",
        when: stamp(m.first_seen),
      });
    });
    return out;
  }, [alarmsResp]);
  const errorCount = messages.filter(m => m.tone === "error").length;

  // Fetch series for the active trend whenever the key changes. Builds the
  // [{label, data, color}] array the chart wants by bucketing raw samples
  // into 24 hourly windows.
  useEffect(() => {
    if (!trendKey) return;
    const cfg = TREND_CONFIGS[trendKey];
    if (!cfg) return;
    let aborted = false;
    setTrendData(null);
    setTrendError(null);
    Promise.all(cfg.sensors.map(s => window.api.getSeries(s.slug, 24)))
      .then(results => {
        if (aborted) return;
        setTrendData(cfg.sensors.map((s, i) => ({
          label: s.label,
          color: s.color,
          // Preserve nulls so the chart renders gaps and min/avg/max
          // calculations can filter them out instead of treating absent
          // samples as 0 °C dives.
          data: window.bucketMinutely(results[i]),
        })));
      })
      .catch((e) => {
        if (aborted) return;
        // Surface an explicit error in the modal instead of falling back to
        // a flat-zero placeholder, which previously rendered a misleading
        // horizontal line at 0 °C.
        console.warn("Trend fetch failed:", e);
        setTrendError(e?.message || String(e));
      });
    return () => { aborted = true; };
  }, [trendKey]);

  // Derive the renderable trend object from the active key + fetched data.
  const trend = useMemo(() => {
    if (!trendKey) return null;
    const cfg = TREND_CONFIGS[trendKey];
    if (!cfg) return null;
    const series = trendData ?? cfg.sensors.map(s => ({
      label: s.label, color: s.color, data: Array(24).fill(0),
    }));
    const allNow = series.map(s => s.data[s.data.length - 1]).filter(v => v != null);
    // If every series' freshest bucket is null (gap hour), render "—" rather
    // than letting toFixed run on null and produce "0.0" / "NaN".
    const now = allNow.length === 0
      ? "—"
      : allNow.length === 1
      ? allNow[0].toFixed(1)
      : allNow.map(v => v.toFixed(1)).join(" / ");
    return { ...cfg, series, now };
  }, [trendKey, trendData]);

  const openTrend = (key) => setTrendKey(key);
  const closeTrend = () => { setTrendKey(null); setTrendData(null); setTrendError(null); };

  // Per-card collapsible-card props. `cardVisibility` defaults to true so
  // older persisted tweak objects don't accidentally hide everything.
  const cardProps = (key) => ({
    collapsible: true,
    collapsed: !!cardCollapse[key],
    onToggleCollapse: () => setCardCollapse(key, !cardCollapse[key]),
  });
  const visible = (key) => cardVis[key] !== false;

  return (
    <div className="app" data-screen-label="01 Dashboard">
      {/* Topbar */}
      <header className="topbar">
        <div className="topbar-lead">
          <Brand state={tempsMeta?.error ? "offline" : (tempsMeta?.loading && !temps) ? "connecting" : "connected"} />
          <div className="topbar-clock" aria-label={`${dateStr} ${clockHM}`}>
            <span className="d">{dateStr}</span>
            <span className="t">{clockHM}</span>
          </div>
        </div>
        <div className="status-strip">
          <div className="sg-wrap">
            {(() => {
              // Treat the backend mode as unknown until /smartgrid has
              // answered successfully at least once. Defaulting to "Normal"
              // silently misrepresents Blocking/LowPrice when the backend is
              // offline or has errored.
              const sgKnown = !!sgResp?.smartgrid_mode && !sgMeta?.error;
              const chipMode = sgKnown ? mode : "normal";
              const chipLabel = sgKnown ? MODE_META[mode].label : "Unknown";
              return (
                <button className={`chip mode mode-${chipMode} sg-trigger`}
                        onClick={() => setSgOpen(o => !o)}
                        aria-expanded={sgOpen}>
                  <span className="label">SmartGrid</span>
                  <span className="value">{chipLabel}</span>
                  <svg width="10" height="10" viewBox="0 0 12 12" fill="none"
                       stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"
                       style={{ marginLeft: 6, opacity: 0.6, transform: sgOpen ? "rotate(180deg)" : "" }}>
                    <path d="M3 4.5l3 3 3-3"/>
                  </svg>
                </button>
              );
            })()}
            {sgOpen && (
              <>
                <div className="sg-back" onClick={() => setSgOpen(false)}/>
                <div className="sg-pop" onClick={e => e.stopPropagation()}>
                  <div className="sg-pop-head">SmartGrid mode</div>
                  {["normal","powersave","lowprice","overcapacity"].map(m => (
                    <button key={m}
                            className={`sg-opt ${mode === m ? "active" : ""} ${MODE_META[m].tone}`}
                            onClick={async () => {
                              setSgOpen(false);
                              if (m === "normal") {
                                if (m === mode) return;
                                try { await window.api.setSmartGridMode("normal", false); } catch (e) { console.error(e); }
                                setMode("normal");
                              } else {
                                // Re-open the confirm dialog even when the
                                // active mode equals the click target, so the
                                // user can pick a different schedule and send
                                // it to the backend.
                                setResumeMode("schedule");
                                setConfirm({ kind: "enter", target: m });
                              }
                            }}>
                      <span className="sg-dot"/>
                      <span className="sg-text">
                        <span className="t">{MODE_META[m].label}</span>
                        <span className="d">{MODE_META[m].desc}</span>
                      </span>
                      {mode === m && (
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                          <path d="M5 12l5 5L20 7"/>
                        </svg>
                      )}
                    </button>
                  ))}
                  {mode !== "normal" && (
                    <button className="sg-exit"
                            onClick={() => { setSgOpen(false); setConfirm({ kind: "exit", target: mode }); }}>
                      Return to Normal
                    </button>
                  )}
                </div>
              </>
            )}
          </div>
          <DhwControl
            dhwResp={dhwResp}
            sysStatus={hp?.sysStatus}
            onRefetch={dhwMeta?.refetch}
            onOpenBath={() => setBathModalOpen(true)}
            onToast={setDhwToast}/>
          {/* "Charging DHW" chip — visible when SystemStatus (62005) == 5
              (heater is currently directing capacity to the DHW tank,
              boost or no boost). Sourced from useHeatPump (existing fetch). */}
          {hp?.sysStatus === 5 && (
            <span className="chip dhw-charging" title="Heater is currently charging the DHW tank">
              <span className="dot"/>
              <span className="value">Charging DHW</span>
            </span>
          )}
          {(() => {
            // DHW boost badge — only renders when an active boost is reported
            // by /api/v1/dhw/state. The badge auto-refreshes on the same 5s
            // cadence as the rest of the dashboard (via useDhwState).
            // remaining_min rounds UP from remaining_s so "1 min" shows until
            // the final tick rather than flashing "0 min" near the end.
            const boost = dhwResp?.boost;
            if (!boost) return null;
            const remSec = Math.max(0, boost.remaining_s ?? 0);
            const remMin = Math.ceil(remSec / 60);
            const presetKind = boost.preset?.kind;
            const txt = presetKind === "shower"
              ? `⚡ DHW Boost · ${remMin} min`
              : (() => {
                  const h = Math.floor(remMin / 60);
                  const m = remMin % 60;
                  const base = `⚡ DHW Boost · ${h} h ${m} min`;
                  return boost.immersion_engaged ? `${base} · ⚙ immersion` : base;
                })();
            return (
              <span id="dhw-boost-badge" className="chip dhw-boost" title="Active DHW boost">
                <span className="value">{txt}</span>
              </span>
            );
          })()}
          {errorCount > 0 && (
            <span className="chip alert-chip"
                  onClick={() => document.getElementById("messages-section")?.scrollIntoView()}
                  title="Active error · click to view">
              <span className="dot"/>
              <span className="label">Error</span>
              <span className="value">{messages.find(m => m.tone === "error").code}</span>
            </span>
          )}
        </div>
      </header>

      {/* Row 0 — Spot Prices · tariff + price level + chart */}
      {visible("prices") && (
      <div className="row full">
        <Card icon="zap" title="Energy & Prices" {...cardProps("prices")}>
          {(() => {
            const tariffMode = gridResp?.tariff_mode;
            const tariffVal  = tariffMode === "high" ? "High"
                              : tariffMode === "low"  ? "Low"
                              : "—";
            const levelLabel = PRICE_LEVEL_LABELS[nowLevel] ?? "—";
            const stats = pricesResp?.today?.spot_statistics;
            const f2v   = v => (v != null ? v.toFixed(2) : "—");
            return (
              <>
                <div className="mode-strip price-strip">
                  <div className="status">
                    <span className="lbl">Grid Tariff</span>
                    <span className="sep">·</span>
                    <span className={`val tariff-${tariffMode || "unknown"}`}>{tariffVal}</span>
                  </div>
                  <div className="status">
                    <span className="lbl">Price Level</span>
                    <span className="sep">·</span>
                    <span className={`val price-level-${nowLevel || "unknown"}`}>{levelLabel}</span>
                  </div>
                  <div className="status">
                    <span className="lbl">Min</span>
                    <span className="sep">·</span>
                    <span className="val">{f2v(stats?.min)}</span>
                  </div>
                  <div className="status">
                    <span className="lbl">Max</span>
                    <span className="sep">·</span>
                    <span className="val">{f2v(stats?.max)}</span>
                  </div>
                  <div className="status">
                    <span className="lbl">Avg</span>
                    <span className="sep">·</span>
                    <span className="val">{f2v(stats?.mean)}</span>
                  </div>
                </div>
                <EnergyChart
                  today={todayPrices}
                  nowIndex={nowIdx}
                  scheduledResumeAt={sgResp?.scheduled_resume_at}
                  scheduledRunMinutes={sgResp?.run_minutes ?? proposedResume?.run_minutes}
                  dhwBoost={dhwResp?.boost}
                  height={narrow ? 140 : 200}
                />
              </>
            );
          })()}
        </Card>
      </div>
      )}

      {/* Row 1 — Heating System (temperatures merged in) */}
      {visible("heatingSystem") && (
      <div className="row full">
        <Card icon="cog" title="Heating System" {...cardProps("heatingSystem")}>
          {(() => {
            const dt = (heating?.flow != null && heating?.ret != null)
              ? heating.flow - heating.ret : null;
            // Heating system status codes: 1, 2 = transitional, 3 = active (no badge), other = off.
            const statusClass = HEATING_STATUS_CLASS[heating?.status] ?? "off";
            return (
              <>
                <div className="mode-strip">
                  <Tip hint="Auto / On / Off · controls heating circuit operation">
                    <div className={`status ${HEATING_MODE_CLASS[heating?.modeLabel] || ""}`}>
                      <span className="lbl">Mode</span>
                      <span className="sep">·</span>
                      <span className="val">{heating?.modeLabel || "—"}</span>
                    </div>
                  </Tip>
                  <Tip hint={whyHeating(heating?.status, temps?.outdoor, temps?.heatOffTemp)}>
                    <div className={`status ${statusClass}`}>
                      <span className="lbl">Status</span>
                      <span className="sep">·</span>
                      <span className="val">{heating?.statusLabel || "—"}</span>
                    </div>
                  </Tip>
                  {pumpResp && !pumpMeta?.error && (() => {
                    // Circulation pump state from the Homey integration. Hidden
                    // when /pump returns 503 (Homey integration disabled).
                    const on = pumpResp.on;
                    const stale = !!pumpResp.stale;
                    const valueLabel = on == null ? "?" : (on ? "On" : "Off");
                    const stamp = pumpResp.last_observed_unix_secs;
                    const ageText = stamp == null
                      ? "not yet observed"
                      : `updated ${Math.max(0, Math.floor(Date.now() / 1000 - stamp))} s ago`;
                    const tip = stale
                      ? `Circulation Pump · Homey unreachable (${ageText})`
                      : `Circulation Pump · ${ageText}`;
                    const cls = stale ? "warn" : on ? "on" : "off";
                    return (
                      <div className={`status ${cls}`} title={tip}>
                        <span className="lbl">Circ. Pump</span>
                        <span className="sep">·</span>
                        <span className="val">{valueLabel}</span>
                      </div>
                    );
                  })()}
                </div>
                <div className="kv-rows">
                  <div className={`kv-row ${t.showSparklines && sparks.upper ? "with-spark" : ""}`}
                       onClick={() => openTrend("dhw")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("dhw"); } }}>
                    <Tip hint="Domestic hot water tank · upper sensor">
                      <span className="k">Upper (DHW)</span>
                    </Tip>
                    <span className="v">
                      {f1(temps?.dhwUpper)}<span className="unit">°C</span>
                      {temps?.dhwStopTemp != null && (
                        <Tip hint="Setpoint (target)"><span className="v-sub">→ {f1(temps.dhwStopTemp)}°C</span></Tip>
                      )}
                    </span>
                    {t.showSparklines && sparks.upper && (
                      <span className="kv-spark"><Sparkline data={sparks.upper} color="var(--temp-dhw-upper)"/></span>
                    )}
                  </div>
                  <div className={`kv-row ${t.showSparklines && sparks.lower ? "with-spark" : ""}`}
                       onClick={() => openTrend("dhw")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("dhw"); } }}>
                    <Tip hint="Radiator buffer tank · lower sensor">
                      <span className="k">Lower (RAD)</span>
                    </Tip>
                    <span className="v">{f1(temps?.lower)}<span className="unit">°C</span></span>
                    {t.showSparklines && sparks.lower && (
                      <span className="kv-spark"><Sparkline data={sparks.lower} color="var(--temp-dhw-lower)"/></span>
                    )}
                  </div>
                  <div className={`kv-row ${t.showSparklines && sparks.room ? "with-spark" : ""}`}
                       onClick={() => openTrend("room")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("room"); } }}>
                    <Tip hint="Indoor room temperature">
                      <span className="k">Room</span>
                    </Tip>
                    <span className="v">
                      {f1(temps?.room)}<span className="unit">°C</span>
                      {temps?.setpoint != null && (
                        <Tip hint="Setpoint (target)"><span className="v-sub">→ {f1(temps.setpoint)}°C</span></Tip>
                      )}
                    </span>
                    {t.showSparklines && sparks.room && (
                      <span className="kv-spark"><Sparkline data={sparks.room} color="var(--temp-room)"/></span>
                    )}
                  </div>
                  <div className={`kv-row ${t.showSparklines && sparks.out ? "with-spark" : ""}`}
                       onClick={() => openTrend("outdoor")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("outdoor"); } }}>
                    <Tip hint="Outdoor air temperature · heat turns off above the threshold">
                      <span className="k">Outdoor</span>
                    </Tip>
                    <span className="v">
                      {f1(temps?.outdoor)}<span className="unit">°C</span>
                      {temps?.heatOffTemp != null && (
                        <Tip hint="Heat-off threshold · heating disables above this outdoor temp"><span className="v-sub">Heat off above {f1(temps.heatOffTemp)}°C</span></Tip>
                      )}
                    </span>
                    {t.showSparklines && sparks.out && (
                      <span className="kv-spark"><Sparkline data={sparks.out} color="var(--temp-outdoor)"/></span>
                    )}
                  </div>
                  <div className={`kv-row ${t.showSparklines && sparks.flow ? "with-spark" : ""}`}
                       onClick={() => setStatsTab("heating")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setStatsTab("heating"); } }}>
                    <Tip hint="Supply flow temperature to the heating circuit · click for trend">
                      <span className="k">Flow</span>
                    </Tip>
                    <span className="v">
                      {f1(heating?.flow)}<span className="unit">°C</span>
                      {heating?.flowSp != null && (
                        <Tip hint="Heat-curve calculated supply target"><span className="v-sub">→ {f1(heating.flowSp)}°C</span></Tip>
                      )}
                    </span>
                    {t.showSparklines && sparks.flow && (
                      <span className="kv-spark"><Sparkline data={sparks.flow} color="var(--temp-flow)"/></span>
                    )}
                  </div>
                  <div className={`kv-row ${t.showSparklines && sparks.ret ? "with-spark" : ""}`}
                       onClick={() => setStatsTab("heating")} role="button" tabIndex={0}
                       onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setStatsTab("heating"); } }}>
                    <Tip hint="Return temperature from the heating circuit · click for trend">
                      <span className="k">Return</span>
                    </Tip>
                    <span className="v">{f1(heating?.ret)}<span className="unit">°C</span></span>
                    {t.showSparklines && sparks.ret && (
                      <span className="kv-spark"><Sparkline data={sparks.ret} color="var(--temp-return)"/></span>
                    )}
                  </div>
                  {!narrow && (
                    <div className="kv-row">
                      <Tip hint="Difference between flow and return">
                        <span className="k">ΔT</span>
                      </Tip>
                      <span className="v">{f1(dt)}<span className="unit">°C</span></span>
                    </div>
                  )}
                </div>
              </>
            );
          })()}
        </Card>
      </div>
      )}

      {/* Row 2 — Heat Pump */}
      {visible("heatPump") && (
      <div className="row full">
        <Card icon="bolt" title="Heat Pump"
              {...cardProps("heatPump")}
              actions={
                <div className="hp-state">
                  <Tip hint="Heat pump state"><Pill tone="on">{hp?.hpStatusLabel || "—"}</Pill></Tip>
                  <Tip hint="Compressor · ON / OFF / WAIT / TEST / DISABLED / ERROR">
                    <Pill tone={hp?.compressor?.tone}>{hp?.compressor?.text || "—"}</Pill>
                  </Tip>
                  <Tip hint="System mode · HP Upper / HP Lower / DHW / HC / Pool / Cooling">
                    <Pill warn>{hp?.sysStatusLabel || "—"}</Pill>
                  </Tip>
                </div>
              }>
          <div className="kv-rows">
            <div className="kv-row" onClick={() => openTrend("hp")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("hp"); } }}>
              <Tip hint="Heat pump inlet temperature (return from system) · click for trend">
                <span className="k">HP In</span>
              </Tip>
              <span className="v">{f1(hp?.hpIn)}<span className="unit">°C</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("hp")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("hp"); } }}>
              <Tip hint="Heat pump outlet temperature (supply to system) · click for trend">
                <span className="k">HP Out</span>
              </Tip>
              <span className="v">{f1(hp?.hpOut)}<span className="unit">°C</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("gas")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("gas"); } }}>
              <Tip hint="Hot gas leaving the compressor · click for trend">
                <span className="k">Discharge</span>
              </Tip>
              <span className="v">{f1(hp?.discharge)}<span className="unit">°C</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("gas")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("gas"); } }}>
              <Tip hint="Cold gas returning to the compressor · click for trend">
                <span className="k">Suction</span>
              </Tip>
              <span className="v">{f1(hp?.suction)}<span className="unit">°C</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("pressure")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("pressure"); } }}>
              <Tip hint="Refrigerant high pressure (condenser side) · click for trend">
                <span className="k">High P</span>
              </Tip>
              <span className="v">{f2(hp?.highP)}<span className="unit">bar</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("pressure")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("pressure"); } }}>
              <Tip hint="Refrigerant low pressure (evaporator side) · click for trend">
                <span className="k">Low P</span>
              </Tip>
              <span className="v">{f2(hp?.lowP)}<span className="unit">bar</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("brine")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("brine"); } }}>
              <Tip hint="Brine loop · collector return → supply · click for trend">
                <span className="k">Brine In</span>
              </Tip>
              <span className="v">{f1(hp?.brineIn)}<span className="unit">°C</span></span>
            </div>
            <div className="kv-row" onClick={() => openTrend("brine")} role="button" tabIndex={0}
                 onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openTrend("brine"); } }}>
              <Tip hint="Brine loop · supply temperature · click for trend">
                <span className="k">
                  Brine Out
                  {hp?.brineDelta != null && (
                    <span className="kv-note">ΔT {f1(hp.brineDelta)}°C</span>
                  )}
                </span>
              </Tip>
              <span className="v">{f1(hp?.brineOut)}<span className="unit">°C</span></span>
            </div>
          </div>
          <div className="pumps">
            <PumpBar label="Charge Pump" pct={Math.round(hp?.chargePump ?? 0)}/>
            <PumpBar label="Brine Pump"  pct={Math.round(hp?.brinePump  ?? 0)}/>
          </div>
        </Card>
      </div>
      )}

      {/* Row 3 — Statistics */}
      {visible("stats") && (
      <div className="row full">
        <Card icon="chart" title="Heat Pump Statistics"
              {...cardProps("stats")}
              banner={(() => {
                const trk = hpStats?.tracking;
                if (!trk) return "heating · — / — starts";
                return `heating · ${(trk.total_operating_hours ?? 0).toFixed(1)}h / ${trk.total_starts ?? 0} starts`;
              })()}
              >
          {(() => {
            const secsToMS = s => {
              if (s == null) return "—";
              const m = Math.floor(s / 60), r = Math.round(s % 60);
              return `${m}:${String(r).padStart(2, "0")}`;
            };
            const hOrMin = h => {
              if (h == null) return "—";
              if (h < 1) return `${Math.round(h * 60)} min`;
              return `${h.toFixed(1)} h`;
            };
            const cs = hpStats?.cycle_stats;
            const starts = hpStats?.starts || {};
            const oh = hpStats?.operating_hours || {};
            return (
          <div className="stats">
            <div className="stat-block">
              <div className="head">
                <span>Cycle Times</span>
                <button className="sb-link" onClick={() => setStatsTab("cycles")} aria-label="Open cycle charts">
                  <Sparkbar data={statsData?.cycleMinutes?.slice(-30)} color="var(--accent)"/>
                </button>
              </div>
              <div className="stat-rows">
                <div className="stat-row"><span className="k">Min</span><span className="v">{secsToMS(cs?.min_secs)}</span></div>
                <div className="stat-row"><span className="k">Max</span><span className="v">{secsToMS(cs?.max_secs)}</span></div>
                <div className="stat-row"><span className="k">Avg</span><span className="v">{secsToMS(cs?.avg_secs)}</span></div>
                <div className="stat-row"><span className="k">Count</span><span className="v">{cs?.cycle_count ?? "—"}</span></div>
              </div>
            </div>
            <div className="stat-block">
              <div className="head">
                <span>Compressor Starts</span>
                <button className="sb-link" onClick={() => setStatsTab("starts")} aria-label="Open starts charts">
                  <Sparkbar data={statsData?.starts?.days?.slice(-30).map(d => d.starts)} color="oklch(0.85 0.13 60)"/>
                </button>
              </div>
              <div className="stat-rows">
                <div className="stat-row"><span className="k">This hour</span><span className="v">{starts.this_hour ?? "—"}</span></div>
                <div className="stat-row"><span className="k">Today</span><span className="v">{starts.this_day ?? "—"}</span></div>
                <div className="stat-row"><span className="k">This week</span><span className="v">{starts.this_week ?? "—"}</span></div>
                <div className="stat-row"><span className="k">This month</span><span className="v">{starts.this_month ?? "—"}</span></div>
                <div className="stat-row featured"><span className="k">This year</span><span className="v">{starts.this_year ?? "—"}</span></div>
              </div>
            </div>
            <div className="stat-block">
              <div className="head">
                <span>Operating Hours</span>
                <button className="sb-link" onClick={() => setStatsTab("hours")} aria-label="Open hours charts">
                  <Sparkbar data={statsData?.days?.slice(-30).map(d => d.hours)} color="var(--cold)"/>
                </button>
              </div>
              <div className="stat-rows">
                <div className="stat-row"><span className="k">This hour</span><span className="v">{hOrMin(oh.this_hour)}</span></div>
                <div className="stat-row"><span className="k">Today</span><span className="v">{hOrMin(oh.this_day)}</span></div>
                <div className="stat-row"><span className="k">This week</span><span className="v">{hOrMin(oh.this_week)}</span></div>
                <div className="stat-row"><span className="k">This month</span><span className="v">{hOrMin(oh.this_month)}</span></div>
                <div className="stat-row featured"><span className="k">This year</span><span className="v">{hOrMin(oh.this_year)}</span></div>
              </div>
            </div>
          </div>
            );
          })()}
        </Card>
      </div>
      )}

      {/* Row 4 — Energy & Prices */}
      {visible("energy") && (
      <div className="row full">
        <Card icon="zap" title="Consumption" {...cardProps("energy")}>
          {(() => {
            const tariffMode = gridResp?.tariff_mode;
            const tariffPill = tariffMode === "high" ? "High Tariff"
                              : tariffMode === "low"  ? "Low Tariff"
                              : "—";
            const currentQuarterKwh = gridResp?.current_quarter_kwh;
            const peakAvg = gridResp?.monthly_peak_avg_kwh;
            // Rolling-quarter kWh history for the inline sparkline. Backend
            // emits up to 96 entries (24 h). Null until /grid lands.
            const quarterSpark = gridResp?.consumption_15min?.length
              ? gridResp.consumption_15min.map(e => e.kwh)
              : null;

            const stats      = pricesResp?.today?.spot_statistics;
            const nowHour    = now.getHours();
            const levelLabel = PRICE_LEVEL_LABELS[nowLevel] ?? "—";

            return (
              <>
                <div className="energy-top">
                  <div className="tariff-block">
                    <span className="lbl">Tariff</span>
                    <span className="tariff-pill">{tariffPill}</span>
                  </div>
                  <div className="divider"/>
                  <div className={`energy-stat ${t.showSparklines && quarterSpark ? "with-spark" : ""}`}>
                    <div className="lbl">Current Quarter</div>
                    <div className="val">{currentQuarterKwh != null ? currentQuarterKwh.toFixed(2) : "—"}<span className="unit">kWh</span></div>
                    {t.showSparklines && quarterSpark && (
                      <span className="es-spark"><Sparkline data={quarterSpark} tone="good"/></span>
                    )}
                  </div>
                  <div className="energy-stat">
                    <div className="lbl">Peak Avg (top 3)</div>
                    <div className="val">{peakAvg != null ? peakAvg.toFixed(2) : "—"}<span className="unit">kWh</span></div>
                  </div>
                </div>

                <div className="price-now">
                  <div className="slot">
                    <div className="slot-lbl">{
                      nowSlot
                        ? `${formatHM(nowSlot.starts_at)} — ${formatHM(nowSlot.ends_at)}`
                        : `${String(nowHour).padStart(2,"0")}:00 — ${String((nowHour+1) % 24).padStart(2,"0")}:00`
                    }</div>
                    <div className="price-big">
                      <span className="num">{nowPrice != null ? nowPrice.toFixed(2) : "—"}</span>
                      <span className="unit">kr/kWh</span>
                      <span className="price-tag">{levelLabel}</span>
                    </div>
                  </div>
                  <div/>
                  <div className="minmax">
                    <div className="it"><div className="l">Min</div><div className="v">{stats?.min != null ? stats.min.toFixed(2) : "—"}</div></div>
                    <div className="it"><div className="l">Max</div><div className="v">{stats?.max != null ? stats.max.toFixed(2) : "—"}</div></div>
                    <div className="it"><div className="l">Avg</div><div className="v">{stats?.mean != null ? stats.mean.toFixed(2) : "—"}</div></div>
                  </div>
                </div>

              </>
            );
          })()}
        </Card>
      </div>
      )}

      {/* Row 5 — Activity Timeline */}
      {visible("activity") && (
      <div className="row full">
        <Card icon="pump" title="Activity · Last 24 h"
              {...cardProps("activity")}
              banner={(() => {
                const segs = activitySegs || [];
                const totalH = segs.reduce((s, seg) => s + (seg.end - seg.start), 0);
                return `compressor on ${totalH.toFixed(1)} h today`;
              })()}>
          <details
            className="chart-collapse"
            open={chartCollapse.activityTimeline}
            onToggle={e => setChartCollapse("activityTimeline", e.currentTarget.open)}
          >
            <summary><span>Compressor activity · 24h</span></summary>
            <ActivityTimeline segments={activitySegs || []} height={narrow ? 48 : 60}/>
            <div className="legend">
              <span className="leg"><i style={{ background: "var(--cat-heating)" }}/> Heating circuit</span>
              <span className="leg"><i style={{ background: "var(--cat-dhw)" }}/> Domestic hot water</span>
              <span className="leg"><i style={{ background: "var(--cat-brine)" }}/> Brine pump</span>
            </div>
          </details>
        </Card>
      </div>
      )}

      {/* Row 7 — Messages */}
      {visible("messages") && (
      <div className="row full" id="messages-section">
        <Card icon="msg" title="System Messages" {...cardProps("messages")}>
          <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
            {messages.length === 0 ? (
              <div className="alert info">
                <div className="glyph"><Icon name="msg" size={16}/></div>
                <div className="body">
                  <div className="top"><span className="code">OK</span>No active messages</div>
                </div>
              </div>
            ) : messages.map((m, i) => (
              <div key={i} className={`alert ${m.tone}`}>
                <div className="glyph"><Icon name={m.tone === "info" ? "msg" : "alert"} size={16}/></div>
                <div className="body">
                  <div className="top">
                    <span className="code">{m.code}</span>
                    {m.title}
                  </div>
                  {m.desc ? <div className="desc">{m.desc}</div> : null}
                </div>
                {m.when ? <div className="when">{m.when}</div> : null}
              </div>
            ))}
          </div>
        </Card>
      </div>
      )}

      <footer className="footer">
        <div>Last updated · {clockStr}</div>
        <div className="right">
          <span className="refresh"><span className="ring"/> Auto-refresh {Math.round(window.POLL_LIVE / 1000)}s</span>
          {version?.version && (
            <>
              <span style={{ color: "var(--line-2)" }}>·</span>
              <span>v{version.version}{version.git_hash ? ` (${version.git_hash})` : ""}</span>
            </>
          )}
        </div>
      </footer>

      {/* Stats charts modal */}
      {statsTab && (
        <div className="scrim" onClick={() => setStatsTab(null)}>
          <div className="trend-modal stats-modal" onClick={e => e.stopPropagation()}>
            <div className="tm-head">
              <div className="tm-title">
                <h3>{STATS_TAB_TITLES[statsTab] ?? "Cycle times"}</h3>
                <div className="tabs" style={{ marginLeft: 4 }}>
                  <button className={`btn ${statsTab === "cycles" ? "active" : ""}`}
                          onClick={() => setStatsTab("cycles")}>Cycles</button>
                  <button className={`btn ${statsTab === "starts" ? "active" : ""}`}
                          onClick={() => setStatsTab("starts")}>Starts</button>
                  <button className={`btn ${statsTab === "hours" ? "active" : ""}`}
                          onClick={() => setStatsTab("hours")}>Operating hours</button>
                  <button className={`btn ${statsTab === "heating" ? "active" : ""}`}
                          onClick={() => setStatsTab("heating")}>Heating system</button>
                </div>
              </div>
              <button className="tm-close" onClick={() => setStatsTab(null)} aria-label="Close">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                  <path d="M6 6l12 12M18 6L6 18"/>
                </svg>
              </button>
            </div>
            {statsTab === "cycles" ? (
              <div className="stats-grid">
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Cycle duration distribution</h4>
                    <span className="ss-sub">{(statsData?.cycleMinutes?.length ?? 0).toLocaleString("sv-SE")} recorded cycles · short cycles signal under-loaded compressor</span>
                  </div>
                  <CycleHistogram data={statsData?.cycleMinutes || undefined}/>
                </div>
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Average cycle length by hour × weekday</h4>
                    <span className="ss-sub">darker = longer cycles · short bands at 06–09 are typical morning DHW draws</span>
                  </div>
                  <HourHeatmap data={statsData?.heat || undefined}/>
                </div>
              </div>
            ) : statsTab === "starts" ? (
              <div className="stats-grid">
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Compressor starts per day · last 60 days</h4>
                    <span className="ss-sub">amber bars exceed 14 starts/day — frequent excursions can wear the compressor</span>
                  </div>
                  <StartsDaily data={statsData?.starts?.days || undefined}/>
                </div>
                <div className="stats-section span-half">
                  <div className="ss-head">
                    <h4>Starts by hour-of-day</h4>
                    <span className="ss-sub">when the compressor most often kicks in</span>
                  </div>
                  <StartsByHour data={statsData?.starts?.byHour || undefined}/>
                </div>
                <div className="stats-section span-half">
                  <div className="ss-head">
                    <h4>Starts vs outdoor temperature</h4>
                    <span className="ss-sub">expected: more starts on cold days — outliers flag faults</span>
                  </div>
                  <StartsVsTemp data={statsData?.starts?.days || undefined}/>
                </div>
              </div>
            ) : statsTab === "heating" ? (
              <div className="stats-grid">
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Flow / Return / ΔT · last 24 h</h4>
                    <span className="ss-sub">filled band = ΔT · narrow band = system in balance · wide band = drawing heat</span>
                  </div>
                  <HeatingTrend data={statsData?.heating || undefined} height={narrow ? 180 : 220}/>
                </div>
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Step response · Flow setpoint → Return temperature</h4>
                    <span className="ss-sub">last 6 setpoint changes overlaid · dashed line = return reaches 90 % of new flow</span>
                  </div>
                  <StepResponse data={stepData}/>
                </div>
              </div>
            ) : (
              <div className="stats-grid">
                <div className="stats-section">
                  <div className="ss-head">
                    <h4>Daily operating hours · last 12 months</h4>
                    <span className="ss-sub">one square per day · darker = more compressor runtime</span>
                  </div>
                  <CalendarHeatmap data={statsData?.days || undefined}/>
                </div>
                <div className="stats-section span-half">
                  <div className="ss-head">
                    <h4>Monthly hours · heating vs hot water</h4>
                    <span className="ss-sub">last 12 months stacked</span>
                  </div>
                  <MonthlyStacked data={statsData?.months || undefined}/>
                </div>
                <div className="stats-section span-half">
                  <div className="ss-head">
                    <h4>Average duty cycle by hour</h4>
                    <span className="ss-sub">24-hour clock · % of each hour the compressor typically runs</span>
                  </div>
                  <RadialClock data={statsData?.radial || undefined}/>
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Trend modal */}
      {trend && (
        <div className="scrim" onClick={() => closeTrend()}>
          <div className="trend-modal" onClick={e => e.stopPropagation()}>
            <div className="tm-head">
              <div className="tm-title">
                <h3>{trend.title}</h3>
                <span className="now">{trend.now}<span className="unit">{trend.unit}</span></span>
              </div>
              <button className="tm-close" onClick={() => closeTrend()} aria-label="Close">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                  <path d="M6 6l12 12M18 6L6 18"/>
                </svg>
              </button>
            </div>
            <div className="tm-sub">{trend.sub} · last 24 h</div>
            {trendError ? (
              <div className="empty-chart">Failed to load trend data: {trendError}</div>
            ) : (
              <>
                <TrendChart series={trend.series} yMin={trend.yMin} yMax={trend.yMax} unit={trend.unit} height={narrow ? 180 : 200}/>
                <div className="tm-stats">
                  {trend.series.map((s, i) => {
                    // Filter null gap hours before computing stats so missing
                    // samples don't pull min/avg toward 0.
                    const vals = s.data.filter(v => v != null);
                    const hasData = vals.length > 0;
                    const min = hasData ? Math.min(...vals) : null;
                    const max = hasData ? Math.max(...vals) : null;
                    const avg = hasData ? vals.reduce((a,b) => a+b, 0) / vals.length : null;
                    return (
                      <div key={i} className="tm-stat">
                        <div className="l">
                          <span style={{display:"inline-block",width:8,height:8,background:s.color,borderRadius:2,marginRight:6}}/>
                          {s.label}
                        </div>
                        <div className="v">{avg != null ? avg.toFixed(1) : "—"}<span className="unit"> avg</span></div>
                        <div className="v" style={{fontSize:12,color:"var(--text-3)"}}>
                          min {min != null ? min.toFixed(1) : "—"} · max {max != null ? max.toFixed(1) : "—"}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>
        </div>
      )}

      {/* SmartGrid mode confirm */}
      {confirm && (() => {
        const target = confirm.target;
        const isEnter = confirm.kind === "enter";
        const meta = MODE_META[target] || MODE_META.normal;
        const isPS = target === "powersave";
        const isLP = target === "lowprice";
        const tone = meta.tone;
        // Clear stale proposedResume so the next open doesn't flash the
        // previous slot before the new fetch lands.
        const closeConfirm = () => { setConfirm(null); setProposedResume(null); setResumeCandidates([]); setPickedResume(0); };
        // Use the backend's proposed auto-resume time directly. The dashboard
        // doesn't recompute it — the backend picks the slot
        // (cheapest_run_within for Blocking, cheap_window_end for
        // LowPrice/Overcapacity) and we display whatever it returns.
        const autoResumeAt = proposedResume?.starts_at
          ? new Date(proposedResume.starts_at).toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false })
          : "—";
        const runMins = sgResp?.run_minutes ?? proposedResume?.run_minutes;
        // Derive the next significant price increase from today's curve.
        // "Significant" = next upcoming 15-min slot priced ≥ 1.5× the current one.
        const priceUp = (() => {
          const prices = pricesResp?.today?.prices || [];
          if (prices.length === 0) return null;
          const curIdx = findPriceSlotAt(prices, now);
          if (curIdx < 0) return null;
          const cur = prices[curIdx]?.spot_sek;
          if (cur == null || cur <= 0) return null;
          for (let i = curIdx + 1; i < prices.length; i++) {
            const p = prices[i]?.spot_sek;
            if (p != null && p >= cur * 1.5) {
              return { at: formatHM(prices[i].starts_at), from: cur, to: p };
            }
          }
          return null;
        })();
        const priceUpAt = priceUp?.at ?? "—";
        const priceUpFrom = priceUp?.from != null ? priceUp.from.toFixed(2) : "—";
        const priceUpTo = priceUp?.to != null ? priceUp.to.toFixed(2) : "—";
        const blurb = isPS ? (
          <>Heating will be blocked while prices are high. Auto-resume time: <strong>{autoResumeAt}</strong>.</>
        ) : isLP ? (
          <>The system will pre-heat the buffer and DHW tank during cheap hours and ease off when prices climb. Auto-resume time: <strong>{autoResumeAt}</strong>.</>
        ) : (
          <>Boost mode — heat aggressively while there is surplus capacity on the grid. Price is forecast to rise from <strong>{priceUpFrom}</strong> to <strong>{priceUpTo} kr/kWh</strong> at <strong>{priceUpAt}</strong>.</>
        );
        const optA = isPS
          ? { t: `Auto-resume at ${autoResumeAt}`, d: runMins
              ? `Heat resumes at the start of the cheapest ${runMins}-minute window in the next few hours.`
              : "Heat will turn back on automatically at the time chosen by the server." }
          : isLP
          ? { t: `Auto-return to Normal at ${autoResumeAt}`, d: "Switches back when the cheap-price window ends." }
          : { t: `Auto-return to Normal at ${priceUpAt}`, d: "Switches back right before the next price increase." };
        const optB = isPS
          ? { t: "Block until I resume manually", d: "Heating stays off until you switch back to Normal." }
          : isLP
          ? { t: "Run until I switch off manually", d: "Stays in Low-price mode until you change it." }
          : { t: "Run until I switch off manually", d: "Stays in Overcapacity until you change it." };
        return (
          <div className="scrim" onClick={() => closeConfirm()}>
            <div className="dialog" onClick={e => e.stopPropagation()}>
              <div className="head">
                <div className={`glyph ${tone}`}><Icon name="bolt" size={16}/></div>
                <h3>{isEnter ? `Enable ${meta.label}?` : `Disable ${meta.label}?`}</h3>
              </div>
              {isEnter ? (
                <>
                  <div className="body">{blurb}</div>
                  {isPS ? (
                    <div className="resume-picker">
                      <div className="resume-picker-label">Resume heating at…</div>
                      {resumeCandidates.length === 0 ? (
                        <div className="resume-empty">No price data — block without auto-resume.</div>
                      ) : resumeCandidates.map((c, i) => {
                        const t0 = new Date(c.starts_at).toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false });
                        const t1 = new Date(c.ends_at).toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false });
                        const ore = Math.round(c.avg_spot_sek * 100);
                        return (
                          <label key={c.starts_at} className={`resume-row level-${c.level || "normal"}`}>
                            <input type="radio" name="resume"
                                   checked={resumeMode === "schedule" && pickedResume === i}
                                   onChange={() => { setResumeMode("schedule"); setPickedResume(i); }}/>
                            <span className="resume-time">{t0}–{t1}{i === 0 ? " · cheapest" : ""}</span>
                            <span className="resume-price">{ore} öre</span>
                          </label>
                        );
                      })}
                      <label className="resume-row manual">
                        <input type="radio" name="resume"
                               checked={resumeMode === "manual"}
                               onChange={() => setResumeMode("manual")}/>
                        <span className="resume-time">Block, no auto-resume</span>
                      </label>
                    </div>
                  ) : (
                    <div className="resume-options">
                      <label className="resume-opt">
                        <input type="radio" name="resume"
                               checked={resumeMode === "schedule"}
                               onChange={() => setResumeMode("schedule")}/>
                        <div>
                          <div className="t">{optA.t}</div>
                          <div className="d">{optA.d}</div>
                        </div>
                      </label>
                      <label className="resume-opt">
                        <input type="radio" name="resume"
                               checked={resumeMode === "manual"}
                               onChange={() => setResumeMode("manual")}/>
                        <div>
                          <div className="t">{optB.t}</div>
                          <div className="d">{optB.d}</div>
                        </div>
                      </label>
                    </div>
                  )}
                  <div className="actions">
                    <button className="btn" onClick={() => closeConfirm()}>Never mind</button>
                    <button className={`btn primary ${tone}`}
                            onClick={async () => {
                              const backendMode = UI_TO_BACKEND_MODE[target] ?? target;
                              const schedule = resumeMode === "schedule";
                              const resumeAt = (schedule && isPS && resumeCandidates[pickedResume])
                                ? resumeCandidates[pickedResume].starts_at
                                : null;
                              try {
                                await window.api.setSmartGridMode(backendMode, schedule, resumeAt);
                                setMode(target);
                              } catch (e) {
                                console.error("SmartGrid POST failed:", e);
                              }
                              closeConfirm();
                            }}>
                      Enable {meta.label}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <div className="body">
                    This will return the system to Normal and cancel any pending auto-schedule.
                  </div>
                  <div className="actions">
                    <button className="btn" onClick={() => closeConfirm()}>Never mind</button>
                    <button className={`btn primary ${tone}`}
                            onClick={async () => {
                              try {
                                await window.api.setSmartGridMode("normal", false);
                                setMode("normal");
                              } catch (e) {
                                console.error("SmartGrid POST failed:", e);
                              }
                              closeConfirm();
                            }}>
                      Return to Normal
                    </button>
                  </div>
                </>
              )}
            </div>
          </div>
        );
      })()}

      <DhwBathModal
        open={bathModalOpen}
        dhwResp={dhwResp}
        onClose={() => setBathModalOpen(false)}
        onApplied={() => { setBathModalOpen(false); dhwMeta?.refetch?.(); }}/>

      {dhwToast && (
        <div className="dhw-toast" role="status" aria-live="polite"
             onClick={() => setDhwToast(null)}>
          {dhwToast}
        </div>
      )}

      {/* Tweaks */}
      <TweaksPanel title="Tweaks">
        <TweakSection title="Theme">
          <TweakRadio label="Mode" value={t.theme}
            onChange={v => setTweak("theme", v)}
            options={[{value:"dark", label:"Dark"},{value:"light", label:"Light"}]}/>
          <TweakColor label="Accent" value={t.accent}
            onChange={v => setTweak("accent", v)}
            options={[
              { value: "mint",  color: ACCENTS.mint.c },
              { value: "ice",   color: ACCENTS.ice.c },
              { value: "amber", color: ACCENTS.amber.c },
              { value: "coral", color: ACCENTS.coral.c },
            ]}/>
        </TweakSection>
        <TweakSection title="Layout">
          <TweakRadio label="Density" value={t.density}
            onChange={v => setTweak("density", v)}
            options={[
              {value:"compact",      label:"Compact"},
              {value:"comfortable",  label:"Comfy"},
              {value:"spacious",     label:"Spacious"},
            ]}/>
          <TweakRadio label="Cards" value={t.cards}
            onChange={v => setTweak("cards", v)}
            options={[
              {value:"outlined", label:"Outline"},
              {value:"flat",     label:"Flat"},
              {value:"elevated", label:"Lifted"},
            ]}/>
        </TweakSection>
        <TweakSection title="Details">
          <TweakToggle label="Sparklines on temperatures"
            value={t.showSparklines}
            onChange={v => setTweak("showSparklines", v)}/>
        </TweakSection>
        <TweakSection title="Cards on screen">
          {[
            ["prices",        "Energy & Prices"],
            ["heatingSystem", "Heating System"],
            ["heatPump",      "Heat Pump"],
            ["stats",         "Statistics"],
            ["energy",        "Consumption"],
            ["activity",      "Activity timeline"],
            ["messages",      "System messages"],
          ].map(([key, label]) => (
            <TweakToggle key={key} label={label}
              value={cardVis[key] !== false}
              onChange={v => setCardVis(key, v)}/>
          ))}
        </TweakSection>
      </TweaksPanel>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(<App/>);
