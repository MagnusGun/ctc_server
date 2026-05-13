/* React hooks wrapping window.api.
   Each hook polls on `intervalMs` (default 5s for live, 5min for prices)
   and returns [data, { error, loading }]. */

const POLL_LIVE = 5_000;
const POLL_PRICES = 5 * 60_000;

// Polled-JSON change detection. JSON.stringify is the simplest stable signature
// for the small (<10 KB) payloads on this dashboard; revisit the cost if a
// future endpoint grows past that.
const sameJson = (a, b) => {
    if (a === b) return true;
    try { return JSON.stringify(a) === JSON.stringify(b); }
    catch { return false; }
};

function usePolledFetch(fetcher, intervalMs) {
    const [data, setData] = React.useState(null);
    const [error, setError] = React.useState(null);
    const [loading, setLoading] = React.useState(true);

    React.useEffect(() => {
        let mounted = true;
        const tick = async () => {
            try {
                const result = await fetcher();
                if (!mounted) return;
                setData(prev => (sameJson(prev, result) ? prev : result));
                setError(null);
            } catch (e) {
                if (!mounted) return;
                setError(e.message || String(e));
            } finally {
                if (mounted) setLoading(false);
            }
        };
        tick();
        const id = setInterval(tick, intervalMs);
        return () => { mounted = false; clearInterval(id); };
    }, [fetcher, intervalMs]);

    return [data, { error, loading }];
}

/* Server-derived 24h activity timeline. Backend returns UTC ISO times;
   we bucket into local-time hours since today's local midnight. JS Date
   arithmetic on UTC instants gives the correct local elapsed hours,
   so an explicit timezone conversion isn't needed. */
const LANE_LABEL = { heating: "Heating", dhw: "DHW", brine: "Brine" };

async function getActivitySegments() {
    const raw = await window.api.getActivity(24);
    // Local midnight today (the underlying instant is correct regardless of TZ).
    const dayStart = new Date();
    dayStart.setHours(0, 0, 0, 0);
    const segments = [];
    for (const s of raw || []) {
        const start = new Date(s.start_iso);
        const end = new Date(s.end_iso);
        if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) continue;
        if (end < dayStart) continue;
        const sH = Math.max(0, (start - dayStart) / 3_600_000);
        const eH = Math.min(24, (end - dayStart) / 3_600_000);
        const lane = LANE_LABEL[s.lane] || s.lane;
        if (eH > sH) segments.push({ lane, start: sH, end: eH });
    }
    return segments;
}

/* Bucket raw [{t, v}] samples into `buckets` hourly windows ending now.
   Each bucket = mean of samples whose timestamps fall inside it. Empty
   buckets stay null so the chart can render gaps where data is missing
   instead of a fake flat line. */
function bucketHourly(points, buckets = 24) {
    if (!points || points.length === 0) {
        return Array(buckets).fill(null);
    }
    const now = Math.floor(Date.now() / 1000);
    const windowStart = now - buckets * 3600;
    const sum = new Array(buckets).fill(0);
    const count = new Array(buckets).fill(0);
    for (const { t, v } of points) {
        // Samples at t === now land at idx === buckets; clamp into the last
        // bucket so the freshest reading isn't dropped.
        let idx = Math.floor((t - windowStart) / 3600);
        if (idx === buckets) idx = buckets - 1;
        if (idx < 0 || idx >= buckets) continue;
        sum[idx] += v;
        count[idx] += 1;
    }
    return sum.map((s, i) => (count[i] > 0 ? s / count[i] : null));
}

const useActivity       = () => usePolledFetch(getActivitySegments, POLL_LIVE);
const useSeries         = (slug, hours = 24) => {
    const fetcher = React.useCallback(
        () => window.api.getSeries(slug, hours), [slug, hours]);
    return usePolledFetch(fetcher, POLL_LIVE);
};
const useHeatPumpHistory = (days = 365) => {
    const fetcher = React.useCallback(
        () => window.api.getHeatPumpHistory(days), [days]);
    return usePolledFetch(fetcher, 60_000);
};
const useStepResponse   = (limit = 6) => {
    const fetcher = React.useCallback(
        () => window.api.getStepResponse(limit), [limit]);
    return usePolledFetch(fetcher, 60_000);
};

/* Transform backend StepEventBlob[] into the shape the StepResponse chart
   expects: [{sp0, sp1, lag, t90, samples: [{t, flow, ret}, ...]}, ...]. */
function transformStepEvents(events) {
    if (!Array.isArray(events) || events.length === 0) return [];
    return events.map(e => {
        const sp0 = e.flow_before;
        const sp1 = e.flow_after;
        const r0 = e.return_before;
        const span = sp1 - r0;
        // lag = first sample where return moves >0.5°C from baseline.
        let lag = null;
        // t90 = first sample where return reaches 90% of the propagation span.
        let t90 = null;
        const samples = (e.samples || []).map(([t, flow, ret]) => {
            if (lag == null && Math.abs(ret - r0) >= 0.5) lag = t;
            if (t90 == null && span !== 0 && (ret - r0) / span >= 0.9) t90 = t;
            return { t, flow, ret };
        });
        return {
            sp0, sp1,
            lag: lag ?? (samples.length ? samples[samples.length - 1].t : 0),
            t90: t90 ?? (samples.length ? samples[samples.length - 1].t : 0),
            samples,
        };
    });
}

/* Convert /heatpump/stats/history + series fetches to the shapes
   the stats-charts.jsx components expect. Before history arrives,
   returns a populated object with empty arrays so consumers can
   render their EmptyChart placeholders without optional-chaining
   on every nested access. */
function buildStatsData(history, flowSeries, retSeries) {
    if (!history) return {
        cycleMinutes: [],
        heat: Array.from({length: 7}, () => Array(24).fill(0)),
        days: [],
        months: [],
        radial: Array(24).fill(0),
        starts: { days: [], byHour: Array(24).fill(0) },
        heating: null,
    };
    const cycles = history.cycles || [];
    const daily  = history.daily  || [];

    // 1. Cycle duration in minutes for CycleHistogram.
    const cycleMinutes = cycles.map(c => (c.duration_secs || 0) / 60);

    // 2. Hour × weekday heatmap (avg cycle minutes per cell).
    // Design choice: cycles are stored as UTC instants on the server but
    // bucketed here using the browser's local timezone (`getHours`/`getDay`).
    // This is intentional — a Swedish user sees hours that align with their
    // wall clock (e.g. "evening DHW draw at 18:00 local") rather than with
    // UTC. Same applies to `startsByHour` and `dutySecs` below.
    const heat = Array.from({length: 7}, () => Array(24).fill(0));
    const heatN = Array.from({length: 7}, () => Array(24).fill(0));
    for (const c of cycles) {
        const d = new Date(c.timestamp);
        if (Number.isNaN(d.getTime())) continue;
        // Mon=0..Sun=6
        const dow = (d.getDay() + 6) % 7;
        const h = d.getHours();
        heat[dow][h] += (c.duration_secs || 0) / 60;
        heatN[dow][h] += 1;
    }
    for (let dow = 0; dow < 7; dow++) {
        for (let h = 0; h < 24; h++) {
            heat[dow][h] = heatN[dow][h] > 0 ? heat[dow][h] / heatN[dow][h] : 0;
        }
    }

    // 3. Calendar heatmap — last 365 days, {date, hours}.
    // The server sends `date` as YYYY-MM-DD in Swedish local time. Parse it
    // back to a local Date by splitting the string (avoids `new Date("YYYY-
    // MM-DD")` which is UTC midnight and can shift days for non-UTC users).
    const parseLocalDate = s => {
        const [y, m, d] = String(s).split("-").map(Number);
        return new Date(y, m - 1, d);
    };
    const byDate = new Map();
    for (const d of daily) {
        byDate.set(d.date, d.operating_hours || 0);
    }
    const days = [];
    // Anchor on the latest server-provided day so the calendar aligns with
    // backend-side "today" regardless of the browser's timezone.
    const lastKey = daily.length > 0 ? daily[daily.length - 1].date : null;
    const anchor = lastKey ? parseLocalDate(lastKey) : new Date();
    for (let i = 364; i >= 0; i--) {
        const d = new Date(anchor);
        d.setDate(anchor.getDate() - i);
        const key = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,"0")}-${String(d.getDate()).padStart(2,"0")}`;
        days.push({ date: d, hours: byDate.get(key) || 0 });
    }

    // 4. Monthly stacked — heat vs DHW. Lane attribution requires Step 11
    // analysis; until per-cycle lane is recorded, put the whole bar in
    // `heat` and leave `dhw` at 0.
    const monthMap = new Map();
    for (const d of daily) {
        const parts = String(d.date).split("-");
        if (parts.length !== 3) continue;
        const ym = `${parts[0]}-${parts[1]}`;
        const cur = monthMap.get(ym) || { hours: 0, year: Number(parts[0]), month: Number(parts[1]) };
        cur.hours += d.operating_hours || 0;
        monthMap.set(ym, cur);
    }
    const months = [];
    const now = new Date();
    for (let i = 11; i >= 0; i--) {
        const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
        const ym = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,"0")}`;
        const entry = monthMap.get(ym);
        months.push({
            label: d.toLocaleDateString("en-GB", { month: "short" }),
            year: d.getFullYear(),
            heat: Math.round(entry?.hours || 0),
            dhw: 0,
        });
    }

    // 5. Radial duty clock — average duty fraction (0..1) per hour
    // computed over the cycle history horizon.
    const dutySecs = Array(24).fill(0);
    const dutySpanDays = Math.max(1, Math.min(30, daily.length || 1));
    for (const c of cycles) {
        const d = new Date(c.timestamp);
        if (Number.isNaN(d.getTime())) continue;
        const h = d.getHours();
        dutySecs[h] += c.duration_secs || 0;
    }
    const radial = dutySecs.map(s => Math.min(1, s / (dutySpanDays * 3600)));

    // 6. Starts data — { days: [{date, starts, outdoor}], byHour: [24] }.
    // The charts call `date.toLocaleDateString()` so we hand them real Dates.
    // Days with null outdoor are dropped so they don't drag the regression
    // toward 0°C.
    const startsDays = daily.slice(-60)
        .filter(d => d.avg_outdoor_temp_c != null)
        .map(d => ({
            date: parseLocalDate(d.date),
            starts: d.starts || 0,
            outdoor: d.avg_outdoor_temp_c,
        }));
    const startsByHour = Array(24).fill(0);
    for (const c of cycles) {
        const d = new Date(c.timestamp);
        if (Number.isNaN(d.getTime())) continue;
        startsByHour[d.getHours()] += 1;
    }

    // 7. Heating trend — 24 hourly buckets of flow + return from the
    // series store. `null` until both fetches land. Per-bucket nulls
    // (sensor offline or cache miss) flow through as `null` so the chart
    // can render gaps rather than misleading zero-dives.
    let heating = null;
    if (flowSeries && retSeries) {
        const flow = window.bucketHourly(flowSeries, 24);
        const ret  = window.bucketHourly(retSeries,  24);
        heating = { flow, ret, events: [] };
    }

    return {
        cycleMinutes,
        heat,
        days,
        months,
        radial,
        starts: { days: startsDays, byHour: startsByHour },
        heating,
    };
}
const useTemperatures   = () => {
    const [data, meta] = usePolledFetch(window.api.getTemperatures, POLL_LIVE);
    // Surface per-source errors so the Connected chip can flip to Offline.
    // errors[] is pre-stringified in api.js so JSON change-detection works.
    const subError = data?.errors?.[0] ?? null;
    return [data, { ...meta, error: meta.error || subError }];
};
const useHeatingSystem  = () => usePolledFetch(window.api.getHeatingSystem, POLL_LIVE);
const useHeatPump       = () => usePolledFetch(window.api.getHeatPump, POLL_LIVE);
const useHeatPumpStats  = () => usePolledFetch(window.api.getHeatPumpStats, POLL_LIVE);
const useAlarms         = () => usePolledFetch(window.api.getAlarms, POLL_LIVE);
const useSmartGrid      = () => usePolledFetch(window.api.getSmartGrid, POLL_LIVE);
const useGrid           = () => usePolledFetch(window.api.getGrid, POLL_LIVE);
const usePrices         = () => usePolledFetch(window.api.getPrices, POLL_PRICES);
const usePump           = () => usePolledFetch(window.api.getPump, POLL_LIVE);

window.POLL_LIVE = POLL_LIVE;
window.POLL_PRICES = POLL_PRICES;
window.bucketHourly = bucketHourly;
window.buildStatsData = buildStatsData;
window.useSeries = useSeries;
window.useHeatPumpHistory = useHeatPumpHistory;
window.useStepResponse = useStepResponse;
window.transformStepEvents = transformStepEvents;
window.useActivity = useActivity;
window.useTemperatures = useTemperatures;
window.useHeatingSystem = useHeatingSystem;
window.useHeatPump = useHeatPump;
window.useHeatPumpStats = useHeatPumpStats;
window.useAlarms = useAlarms;
window.useSmartGrid = useSmartGrid;
window.useGrid = useGrid;
window.usePrices = usePrices;
window.usePump = usePump;
