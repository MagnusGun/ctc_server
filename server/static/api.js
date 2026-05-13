/* Thin fetch layer for the dashboard.
   No build step — plain JS so it's available before any JSX runs.
   Register addresses mirror the ones the previous app.js used. */

const API_BASE = '/api/v1';

const REG = {
    DHW_UPPER:        { addr: 62276, factor: 0.1 }, // DHW actual temp
    LOWER_TANK:       { addr: 62006, factor: 0.1 }, // Lower tank / radiator
    HEATING_MODE:     { addr: 61542, factor: 1   }, // 0 Auto, 1 On, 2 Off
    HEATING_STATUS:   { addr: 62246, factor: 1   }, // 0 Off, 1 Vacation, 2 Night, 3 Normal
    FLOW_SETPOINT:    { addr: 62007, factor: 0.1 }, // Heating system 1 supply-flow setpoint (heat curve)
    HP_STATUS:        { addr: 62017, factor: 1   },
    SYSTEM_STATUS:    { addr: 62005, factor: 1   },
    HP_IN:            { addr: 62027, factor: 0.1 },
    HP_OUT:           { addr: 62037, factor: 0.1 },
    HP_DISCHARGE:     { addr: 62047, factor: 0.1 },
    HP_SUCTION:       { addr: 62057, factor: 0.1 },
    HP_HIGH_P:        { addr: 62067, factor: 0.1 }, // bar
    HP_LOW_P:         { addr: 62077, factor: 0.1 }, // bar
    BRINE_IN:         { addr: 62087, factor: 0.1 },
    BRINE_OUT:        { addr: 62097, factor: 0.1 },
    CHARGE_PUMP_PCT:  { addr: 62107, factor: 0.1 },
    BRINE_PUMP_PCT:   { addr: 62117, factor: 0.1 },
};

const HEATING_MODE_LABELS = { 0: 'Auto', 1: 'On', 2: 'Off' };
const HEATING_STATUS_LABELS = { 0: 'Off', 1: 'Vacation', 2: 'Night', 3: 'Normal' };
const HP_STATUS_LABELS = {
    0: 'Start Delay', 1: 'Ready', 2: 'Wait Flow', 3: 'Heating',
    4: 'Defrost', 5: 'Cooling', 6: 'Blocked', 7: 'Alarm', 8: 'Test',
    30: 'Not Defined', 31: 'Not Enabled', 32: 'Comm Error', 33: 'Charge DHW',
};
const SYSTEM_STATUS_LABELS = {
    0: 'HP Upper', 1: 'HP Lower', 2: 'Add', 3: 'HP+Add', 4: 'HC',
    5: 'DHW', 6: 'Pool', 7: 'Off', 8: 'Heat Mix', 9: 'Wood',
    10: 'DHW/HC', 11: 'Cooling', 12: 'Swap',
};

function compressorState(statusCode) {
    switch (statusCode) {
        case 3: case 4: case 5: return { text: 'ON', tone: 'on' };
        case 2:                 return { text: 'WAIT', tone: 'wait' };
        case 8:                 return { text: 'TEST', tone: 'test' };
        case 0: case 1: case 6: case 7: return { text: 'OFF', tone: 'off' };
        case 30: case 33:       return { text: 'N/A', tone: 'na' };
        case 31:                return { text: 'DISABLED', tone: 'disabled' };
        case 32:                return { text: 'ERROR', tone: 'error' };
        default:                return { text: '?', tone: '' };
    }
}

async function fetchJson(url) {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`HTTP ${r.status} on ${url}`);
    return r.json();
}

async function readRegister({ addr, factor }) {
    const data = await fetchJson(`${API_BASE}/ctc?addr=${addr}&factor=${factor}`);
    return data.ctc_data;
}

const settledValue = r => (r.status === 'fulfilled' ? r.value : null);
const settledError = r => (r.status === 'rejected'  ? r.reason : null);

/* High-level fetchers — one per card. Each returns a plain object the card consumes. */

async function getTemperatures() {
    const [room, outdoor, setpoint, dhwUpper, lower] = await Promise.allSettled([
        fetchJson(`${API_BASE}/temperature/room`).then(d => d.room_temperature),
        fetchJson(`${API_BASE}/temperature/outdoor`).then(d => d.outdoor_temperature),
        fetchJson(`${API_BASE}/temperature/room/setpoint`).then(d => d.room_temperature_setpoint),
        readRegister(REG.DHW_UPPER),
        readRegister(REG.LOWER_TANK),
    ]);
    // Plain string messages so usePolledFetch's JSON.stringify change-detection
    // can compare them; Error instances serialize to {} and look identical even
    // when the underlying message differs.
    const errors = [room, outdoor, setpoint, dhwUpper, lower]
        .map(settledError).filter(e => e != null)
        .map(e => e?.message ?? String(e));
    return {
        room: settledValue(room),
        outdoor: settledValue(outdoor),
        setpoint: settledValue(setpoint),
        dhwUpper: settledValue(dhwUpper),
        lower: settledValue(lower),
        errors,
    };
}

async function getHeatingSystem() {
    const [mode, status, flow, ret, flowSp] = await Promise.allSettled([
        readRegister(REG.HEATING_MODE),
        readRegister(REG.HEATING_STATUS),
        fetchJson(`${API_BASE}/temperature/flow`).then(d => d.flow_outlet_temperature),
        fetchJson(`${API_BASE}/temperature/flow/return`).then(d => d.flow_return_temperature),
        readRegister(REG.FLOW_SETPOINT),
    ]);
    const modeVal = settledValue(mode);
    const statusVal = settledValue(status);
    return {
        mode: modeVal,
        modeLabel: HEATING_MODE_LABELS[modeVal],
        status: statusVal,
        statusLabel: HEATING_STATUS_LABELS[statusVal],
        flow: settledValue(flow),
        ret: settledValue(ret),
        flowSp: settledValue(flowSp),
    };
}

async function getHeatPump() {
    const keys = ['hpStatus', 'sysStatus', 'hpIn', 'hpOut', 'discharge', 'suction',
                  'highP', 'lowP', 'brineIn', 'brineOut', 'chargePump', 'brinePump'];
    const regs = [REG.HP_STATUS, REG.SYSTEM_STATUS, REG.HP_IN, REG.HP_OUT,
                  REG.HP_DISCHARGE, REG.HP_SUCTION, REG.HP_HIGH_P, REG.HP_LOW_P,
                  REG.BRINE_IN, REG.BRINE_OUT, REG.CHARGE_PUMP_PCT, REG.BRINE_PUMP_PCT];
    const results = await Promise.allSettled(regs.map(readRegister));
    const out = {};
    results.forEach((r, i) => { out[keys[i]] = settledValue(r); });
    out.hpStatusLabel = HP_STATUS_LABELS[out.hpStatus];
    out.sysStatusLabel = SYSTEM_STATUS_LABELS[out.sysStatus];
    out.compressor = compressorState(out.hpStatus);
    if (out.brineIn != null && out.brineOut != null) {
        out.brineDelta = out.brineIn - out.brineOut;
    }
    return out;
}

async function getHeatPumpStats() { return fetchJson(`${API_BASE}/heatpump/stats`); }
async function getHeatPumpHistory(days = 30) {
    return fetchJson(`${API_BASE}/heatpump/stats/history?days=${days}`);
}
async function getSeries(slug, hours = 24) {
    return fetchJson(`${API_BASE}/heatpump/series?sensor=${slug}&hours=${hours}`);
}
async function getActivity(hours = 24) {
    return fetchJson(`${API_BASE}/heatpump/activity?hours=${hours}`);
}
async function getStepResponse(limit = 6) {
    return fetchJson(`${API_BASE}/heatpump/step_response?limit=${limit}`);
}

async function getAlarms() {
    return fetchJson(`${API_BASE}/alarms`);
}

async function getSmartGrid() { return fetchJson(`${API_BASE}/smartgrid`); }
async function getSmartGridResume() {
    try { return await fetchJson(`${API_BASE}/smartgrid/proposed_resume`); }
    catch { return null; }
}

async function setSmartGridMode(mode, scheduleResume = false) {
    const url = `${API_BASE}/smartgrid?mode=${mode}` +
                (scheduleResume ? '&schedule_resume=true' : '');
    const r = await fetch(url, { method: 'POST' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
}

async function getGrid()   { return fetchJson(`${API_BASE}/grid`); }
async function getPrices() { return fetchJson(`${API_BASE}/prices`); }

// Cirkulationspump state via Homey. 503 means the Homey integration is
// disabled — the badge should hide itself when this resolves to null.
async function getPump() {
    try { return await fetchJson(`${API_BASE}/pump`); }
    catch { return null; }
}

async function getVersion() { return fetchJson(`${API_BASE}/version`); }

window.api = {
    REG, fetchJson, readRegister,
    getTemperatures, getHeatingSystem, getHeatPump,
    getHeatPumpStats, getHeatPumpHistory,
    getSeries, getActivity, getStepResponse,
    getAlarms,
    getSmartGrid, getSmartGridResume, setSmartGridMode,
    getGrid, getPrices, getPump,
    getVersion,
};
