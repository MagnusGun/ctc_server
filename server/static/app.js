// CTC EcoHeat 400 Dashboard - Data Fetching and Display

const API_BASE = '/api/v1';
const REFRESH_INTERVAL = 5000; // 5 seconds

// Status mappings
const HEATING_MODES = { 0: 'Auto', 1: 'On', 2: 'Off' };
const HEATING_STATUS = { 0: 'Off', 1: 'Vacation', 2: 'Night', 3: 'Normal' };
const HP_STATUS = {
    0: 'Start Delay',    // Compressor_off_start_delay
    1: 'Ready',          // Compressor_off_ready_to_start
    2: 'Wait Flow',      // Compressor_wait_until_flow
    3: 'Heating',        // Compressor_on_heating
    4: 'Defrost',        // Defrost_active
    5: 'Cooling',        // Compressor_on_cooling
    6: 'Blocked',        // Compressor_off_blocked
    7: 'Alarm',          // Compressor_off_alarm
    8: 'Test',           // Function_test
    30: 'Not Defined',   // HP not defined
    31: 'Not Enabled',   // Compressor not enabled
    32: 'Comm Error',    // Communication error
    33: 'Charge DHW'     // Charge dhw
};
const SYSTEM_STATUS = {
    0: 'HP Upper', 1: 'HP Lower', 2: 'Add', 3: 'HP+Add', 4: 'HC',
    5: 'DHW', 6: 'Pool', 7: 'Off', 8: 'Heat Mix', 9: 'Wood',
    10: 'DHW/HC', 11: 'Cooling', 12: 'Swap'
};

// Get compressor state from HP status code
function getCompressorState(statusCode) {
    switch (statusCode) {
        case 3:  // Heating
        case 4:  // Defrost
        case 5:  // Cooling
            return { text: 'ON', class: 'on' };
        case 2:  // Wait for flow
            return { text: 'WAIT', class: 'wait' };
        case 8:  // Function test
            return { text: 'TEST', class: 'test' };
        case 0:  // Start delay
        case 1:  // Ready
        case 6:  // Blocked
        case 7:  // Alarm
            return { text: 'OFF', class: 'off' };
        case 30: // HP not defined
        case 33: // Charge DHW (unknown)
            return { text: 'N/A', class: 'na' };
        case 31: // Compressor not enabled
            return { text: 'DISABLED', class: 'disabled' };
        case 32: // Communication error
            return { text: 'ERROR', class: 'error' };
        default:
            return { text: '?', class: '' };
    }
}

// Elements cache
const elements = {};

// State tracking
const state = {
    smartgridMode: null,
    powersaveToggling: false,
    powersaveState: null
};

// Chart hover state
const chartState = {
    hoverIndex: null,
    prices: [],
    padding: null,
    pointWidth: null,
    chartWidth: null,
    chartHeight: null,
    minPrice: null,
    priceRange: null
};

// Heat pump stats state
const hpStatsState = {
    chart: null,
    currentChartType: 'cycles',
    historyData: null
};

// Initialize element references
function initElements() {
    const ids = [
        'room-temp', 'room-setpoint', 'outdoor-temp', 'flow-temp', 'return-temp',
        'dhw-temp', 'lower-tank-temp', 'heating-mode', 'heating-status', 'flow-setpoint',
        'hp-status-badge', 'compressor-badge', 'system-status-badge', 'hp-in', 'hp-out', 'hp-discharge', 'hp-suction',
        'hp-high-p', 'hp-low-p', 'brine-in', 'brine-out', 'brine-delta',
        'charge-pump', 'charge-pump-bar', 'brine-pump', 'brine-pump-bar',
        'alarms-content', 'alarms-panel', 'connection-status', 'last-updated',
        'smartgrid-status', 'smartgrid-timestamp', 'powersave-status', 'powersave-toggle',
        'tariff-mode', 'current-hour-kwh', 'monthly-peak-kwh', 'peak-popup', 'peak-stat',
        // Price elements
        'price-time', 'price-spot', 'price-level', 'price-spot-detail',
        'price-tibber-total', 'price-markup', 'tibber-section',
        'price-min', 'price-max', 'price-avg', 'price-chart', 'chart-tooltip',
        // Heat pump stats elements
        'heatpump-stats-panel', 'hp-tracking-info',
        'cycle-min', 'cycle-max', 'cycle-avg', 'cycle-count',
        'starts-hour', 'starts-day', 'starts-week', 'starts-month', 'starts-year',
        'ophours-hour', 'ophours-day', 'ophours-week', 'ophours-month', 'ophours-year',
        'stats-modal', 'stats-modal-close', 'stats-chart', 'chart-empty'
    ];
    ids.forEach(id => {
        elements[id] = document.getElementById(id);
    });
}

// Format temperature value
function formatTemp(value, decimals = 1) {
    if (value === null || value === undefined || isNaN(value)) return '--.-°';
    return `${value.toFixed(decimals)}°`;
}

// Format pressure value
function formatPressure(value) {
    if (value === null || value === undefined || isNaN(value)) return '--.-- bar';
    return `${value.toFixed(2)} bar`;
}

// Format percentage
function formatPercent(value) {
    if (value === null || value === undefined || isNaN(value)) return '--%';
    return `${Math.round(value)}%`;
}

// Format duration in seconds as MM:SS or HH:MM:SS
function formatDuration(secs) {
    if (secs === null || secs === undefined || isNaN(secs)) return '--:--';
    const hours = Math.floor(secs / 3600);
    const mins = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    if (hours > 0) {
        return `${hours}:${mins.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
    }
    return `${mins}:${s.toString().padStart(2, '0')}`;
}

// Fetch JSON from API
async function fetchJson(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
}

// Fetch a CTC register value
async function fetchRegister(addr, factor = 0.1) {
    const data = await fetchJson(`${API_BASE}/ctc?addr=${addr}&factor=${factor}`);
    return data.ctc_data;
}

// Update connection status
function setConnectionStatus(status) {
    const el = elements['connection-status'];
    el.className = 'connection-status ' + status;
    el.querySelector('.status-text').textContent =
        status === 'connected' ? 'Connected' :
        status === 'error' ? 'Connection Lost' : 'Connecting...';
}

// Update last updated time
function updateTimestamp() {
    const now = new Date();
    elements['last-updated'].textContent = now.toLocaleTimeString();
}

// Fetch and update temperature data
async function updateTemperatures() {
    try {
        const [room, outdoor, flow, returnTemp, setpoint] = await Promise.all([
            fetchJson(`${API_BASE}/temperature/room`),
            fetchJson(`${API_BASE}/temperature/outdoor`),
            fetchJson(`${API_BASE}/temperature/flow`),
            fetchJson(`${API_BASE}/temperature/flow/return`),
            fetchJson(`${API_BASE}/temperature/room/setpoint`)
        ]);

        elements['room-temp'].textContent = formatTemp(room.room_temperature);
        elements['outdoor-temp'].textContent = formatTemp(outdoor.outdoor_temperature);
        elements['flow-temp'].textContent = formatTemp(flow.flow_outlet_temperature);
        elements['return-temp'].textContent = formatTemp(returnTemp.flow_return_temperature);
        elements['room-setpoint'].textContent = `set: ${formatTemp(setpoint.room_setpoint, 0)}`;
    } catch (err) {
        console.error('Error fetching temperatures:', err);
        throw err;
    }
}

// Fetch and update tank temperatures
async function updateTankTemps() {
    try {
        const [dhw, lowerTank] = await Promise.all([
            fetchRegister(62276, 0.1),  // DHW actual temp
            fetchRegister(62006, 0.1)   // Lower tank / radiator water
        ]);

        elements['dhw-temp'].textContent = formatTemp(dhw) + 'C';
        elements['lower-tank-temp'].textContent = formatTemp(lowerTank) + 'C';
    } catch (err) {
        console.error('Error fetching tank temps:', err);
    }
}

// Fetch and update heating system status
async function updateHeatingSystem() {
    try {
        const [mode, status, flowSp] = await Promise.all([
            fetchRegister(61542, 1),    // Heating mode
            fetchRegister(62246, 1),    // Heating system status
            fetchRegister(62007, 0.1)   // Flow setpoint
        ]);

        elements['heating-mode'].textContent = HEATING_MODES[mode] || mode;

        const statusEl = elements['heating-status'];
        const statusText = HEATING_STATUS[status] || String(status || '--');
        statusEl.textContent = statusText;
        statusEl.className = 'info-value status-indicator ' + (statusText || '').toLowerCase();

        elements['flow-setpoint'].textContent = formatTemp(flowSp) + 'C';
    } catch (err) {
        console.error('Error fetching heating system:', err);
    }
}

// Fetch and update heat pump data
async function updateHeatPump() {
    try {
        const [hpStatus, sysStatus, hpIn, hpOut, discharge, suction, highP, lowP, brineIn, brineOut, chargePump, brinePump] = await Promise.all([
            fetchRegister(62017, 1),    // HP status
            fetchRegister(62005, 1),    // System status
            fetchRegister(62027, 0.1),  // HP in
            fetchRegister(62037, 0.1),  // HP out
            fetchRegister(62047, 0.1),  // Discharge
            fetchRegister(62057, 0.1),  // Suction
            fetchRegister(62067, 0.1),  // High pressure
            fetchRegister(62077, 0.1),  // Low pressure
            fetchRegister(62087, 0.1),  // Brine in
            fetchRegister(62097, 0.1),  // Brine out
            fetchRegister(62107, 0.1),  // Charge pump
            fetchRegister(62117, 0.1)   // Brine pump
        ]);

        // HP Status badge
        const statusBadge = elements['hp-status-badge'];
        const statusText = HP_STATUS[hpStatus] || hpStatus;
        statusBadge.textContent = statusText;
        statusBadge.className = 'status-badge' + (hpStatus === 0 ? ' off' : '');

        // Compressor state badge
        const compressorBadge = elements['compressor-badge'];
        const compState = getCompressorState(hpStatus);
        compressorBadge.textContent = compState.text;
        compressorBadge.className = 'status-badge compressor ' + compState.class;

        // System Status badge
        const sysBadge = elements['system-status-badge'];
        const sysText = SYSTEM_STATUS[sysStatus] || sysStatus;
        sysBadge.textContent = sysText;
        sysBadge.className = 'status-badge system' + (sysStatus === 7 ? ' off' : '');

        // Temperatures
        elements['hp-in'].textContent = formatTemp(hpIn);
        elements['hp-out'].textContent = formatTemp(hpOut);
        elements['hp-discharge'].textContent = formatTemp(discharge);
        elements['hp-suction'].textContent = formatTemp(suction);

        // Pressures
        elements['hp-high-p'].textContent = formatPressure(highP);
        elements['hp-low-p'].textContent = formatPressure(lowP);

        // Brine temps and delta (inline display, no degree symbol)
        const formatBrineTemp = (v) => (v === null || v === undefined || isNaN(v)) ? '--.-' : v.toFixed(1);
        elements['brine-in'].textContent = formatBrineTemp(brineIn);
        elements['brine-out'].textContent = formatBrineTemp(brineOut);
        const brineDelta = (brineIn !== null && brineOut !== null) ? brineIn - brineOut : null;
        elements['brine-delta'].textContent = formatBrineTemp(brineDelta);

        // Pump percentages
        elements['charge-pump'].textContent = formatPercent(chargePump);
        elements['charge-pump-bar'].style.width = `${Math.min(100, Math.max(0, chargePump || 0))}%`;

        elements['brine-pump'].textContent = formatPercent(brinePump);
        elements['brine-pump-bar'].style.width = `${Math.min(100, Math.max(0, brinePump || 0))}%`;
    } catch (err) {
        console.error('Error fetching heat pump data:', err);
    }
}

// Fetch and update alarms
async function updateAlarms() {
    try {
        const alarmStatus = await fetchJson(`${API_BASE}/alarms/status`);
        const panel = elements['alarms-panel'];
        const content = elements['alarms-content'];

        if (alarmStatus.alarm_count === 0 && alarmStatus.info_count === 0) {
            panel.classList.remove('has-alarms', 'has-infos');
            content.innerHTML = '<div class="no-alarms">✓ No messages</div>';
        } else {
            // Fetch full alarm details
            const alarmsData = await fetchJson(`${API_BASE}/alarms`);

            // Only show red for actual alarms, blue for info-only
            const hasAlarms = alarmsData.alarms && alarmsData.alarms.length > 0;
            const hasInfos = alarmsData.infos && alarmsData.infos.length > 0;
            panel.classList.toggle('has-alarms', hasAlarms);
            panel.classList.toggle('has-infos', !hasAlarms && hasInfos);

            let html = '';
            if (hasAlarms) {
                alarmsData.alarms.forEach(alarm => {
                    const code = alarm.code;
                    // Use first_seen from server (ISO 8601 format)
                    const timestamp = new Date(alarm.first_seen).toLocaleTimeString();
                    const title = alarm.message_en || alarm.message;
                    const desc = alarm.description ? `<div class="alarm-desc">${alarm.description}</div>` : '';
                    html += `<div class="alarm-item"><span class="alarm-time">${timestamp}</span> ${code}: ${title}${desc}</div>`;
                });
            }
            if (hasInfos) {
                alarmsData.infos.forEach(info => {
                    const code = info.code;
                    // Use first_seen from server (ISO 8601 format)
                    const timestamp = new Date(info.first_seen).toLocaleTimeString();
                    const title = info.message_en || info.message;
                    const desc = info.description ? `<div class="alarm-desc">${info.description}</div>` : '';
                    html += `<div class="alarm-item info"><span class="alarm-time">${timestamp}</span> ${code}: ${title}${desc}</div>`;
                });
            }

            content.innerHTML = html || '<div class="no-alarms">✓ No messages</div>';
        }
    } catch (err) {
        console.error('Error fetching alarms:', err);
    }
}

// Fetch and update grid information (tariff, consumption, peaks)
async function updateGrid() {
    try {
        const data = await fetchJson(`${API_BASE}/grid`);

        // Tariff mode badge
        const tariffEl = elements['tariff-mode'];
        if (tariffEl) {
            const mode = data.tariff_mode || 'Unknown';
            const isHigh = mode.toLowerCase() === 'high';
            tariffEl.textContent = isHigh ? 'High Tariff' : 'Low Tariff';
            tariffEl.className = 'tariff-badge ' + mode.toLowerCase();
        }

        // Current hour consumption with warning indicator
        const currentHourEl = elements['current-hour-kwh'];
        if (currentHourEl) {
            const currentKwh = data.current_hour_kwh;
            const peakAvg = data.monthly_peak_avg_kwh;

            if (currentKwh !== null && currentKwh !== undefined) {
                currentHourEl.textContent = `${currentKwh.toFixed(2)} kWh`;

                // Color coding based on peak comparison
                currentHourEl.classList.remove('warning', 'danger');
                if (peakAvg > 0) {
                    if (currentKwh > peakAvg) {
                        currentHourEl.classList.add('danger');  // Red: exceeding peak
                    } else if (currentKwh >= peakAvg * 0.8) {
                        currentHourEl.classList.add('warning'); // Amber: approaching peak
                    }
                }
            } else {
                currentHourEl.textContent = '-- kWh';
                currentHourEl.classList.remove('warning', 'danger');
            }
        }

        // Monthly peak average with incomplete data indicator
        const peakEl = elements['monthly-peak-kwh'];
        const peakStatEl = elements['peak-stat'];
        if (peakEl) {
            peakEl.textContent = `${data.monthly_peak_avg_kwh.toFixed(2)} kWh`;

            // Show amber color if < 3 days of data
            const recordedDays = data.recorded_days || 0;
            if (recordedDays < 3 && recordedDays > 0) {
                peakEl.classList.add('incomplete');
                if (peakStatEl) {
                    peakStatEl.setAttribute('data-tooltip',
                        `Based on ${recordedDays} day${recordedDays === 1 ? '' : 's'} (need 3 for full accuracy)`);
                }
            } else {
                peakEl.classList.remove('incomplete');
                if (peakStatEl) {
                    peakStatEl.setAttribute('data-tooltip',
                        'Average of top 3 daily peaks this month (one peak per day)');
                }
            }
        }

        // Update peak hours popup
        updatePeakPopup(data.monthly_peak_hours);
    } catch (err) {
        console.error('Error fetching grid data:', err);
    }
}

// Update peak hours popup with bar chart
function updatePeakPopup(peakHours) {
    const popup = elements['peak-popup'];
    if (!popup) return;

    if (!peakHours || peakHours.length === 0) {
        popup.innerHTML = '<div class="peak-no-data">No peak data yet</div>';
        return;
    }

    const maxKwh = Math.max(...peakHours.map(h => h.kwh));

    popup.innerHTML = peakHours.map(hour => {
        const pct = (hour.kwh / maxKwh * 100).toFixed(0);
        const date = new Date(hour.timestamp);
        const dateStr = date.toLocaleDateString('en-GB', {
            month: 'short', day: 'numeric'
        });
        const hourStr = date.toLocaleTimeString('en-GB', {
            hour: '2-digit', minute: '2-digit'
        });
        return `
            <div class="peak-bar">
                <span class="peak-bar-label">${dateStr} ${hourStr}</span>
                <div class="peak-bar-track">
                    <div class="peak-bar-fill" style="width: ${pct}%"></div>
                </div>
                <span class="peak-bar-value">${hour.kwh.toFixed(2)} kWh</span>
            </div>
        `;
    }).join('');
}

// Fetch and update system status (SmartGrid and Powersave)
async function updateSystemStatus() {
    try {
        const [smartgrid, powersave] = await Promise.all([
            fetchJson(`${API_BASE}/smartgrid`),
            fetchJson(`${API_BASE}/ctc/powersave`)
        ]);

        // SmartGrid status
        const sgEl = elements['smartgrid-status'];
        const sgTimestamp = elements['smartgrid-timestamp'];
        if (sgEl) {
            const mode = smartgrid.smartgrid_mode || 'Unknown';
            sgEl.textContent = mode;
            sgEl.className = 'header-badge' + (mode === 'Normal' ? '' : ' active');
            state.smartgridMode = mode;

            // Prefer the scheduled-resume time when one is pending; fall back
            // to the last-changed timestamp otherwise.
            if (sgTimestamp) {
                if (smartgrid.scheduled_resume_at) {
                    const t = new Date(smartgrid.scheduled_resume_at);
                    sgTimestamp.textContent = `auto-resume ${t.toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'})}`;
                } else if (smartgrid.changed_at) {
                    sgTimestamp.textContent = new Date(smartgrid.changed_at).toLocaleTimeString();
                } else {
                    sgTimestamp.textContent = '';
                }
            }
        }

        // Powersave status (skip if toggle in progress to avoid flicker)
        const psEl = elements['powersave-status'];
        if (psEl && !state.powersaveToggling) {
            const active = powersave.powersave;
            state.powersaveState = active;
            psEl.textContent = active ? 'Active' : 'Off';
            psEl.className = 'header-badge' + (active ? ' active' : '');
        }
    } catch (err) {
        console.error('Error fetching system status:', err);
    }
}

// Toggle powersave mode.
// When activating, ask the server for the cheapest 15-min slot in the next
// configured window and let the user confirm whether to schedule an
// auto-resume at that slot (OK = schedule, Cancel = block without schedule).
async function togglePowersave() {
    if (state.powersaveToggling) return;

    const toggleEl = elements['powersave-toggle'];
    const statusEl = elements['powersave-status'];
    if (!toggleEl || !statusEl) return;

    const newActive = !state.powersaveState;

    let scheduleResume = false;
    if (newActive) {
        // Try to fetch the proposed resume slot — non-fatal if it fails.
        let proposed = null;
        try {
            proposed = await fetchJson(`${API_BASE}/smartgrid/proposed_resume`);
        } catch (err) {
            console.warn('Could not fetch proposed resume:', err);
        }

        if (proposed && proposed.starts_at) {
            const t = new Date(proposed.starts_at);
            const when = t.toLocaleString([], {dateStyle: 'short', timeStyle: 'short'});
            const price = (typeof proposed.spot_sek === 'number')
                ? proposed.spot_sek.toFixed(3) + ' SEK/kWh'
                : 'unknown price';
            const msg = `Block the heater and auto-resume at ${when} (${price})?\n\n` +
                        `OK = schedule auto-resume.\nCancel = block without auto-resume.`;
            // OK -> schedule, Cancel -> block-only. Either way, proceed.
            scheduleResume = confirm(msg);
        } else {
            // No price data available — fall back to a plain confirm.
            const msg = `Block the heater? (No price data available — cannot schedule auto-resume.)`;
            if (!confirm(msg)) return;
        }
    } else {
        if (!confirm('Disable power saving mode?')) return;
    }

    state.powersaveToggling = true;
    toggleEl.classList.add('loading');
    toggleEl.classList.remove('error');

    try {
        const url = `${API_BASE}/ctc/powersave?active=${newActive}` +
                    (newActive && scheduleResume ? '&schedule_resume=true' : '');
        const response = await fetch(url, { method: 'POST' });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);

        const data = await response.json();
        state.powersaveState = data.powersave;
        statusEl.textContent = data.powersave ? 'Active' : 'Off';
        statusEl.className = 'header-badge' + (data.powersave ? ' active' : '');

        // Surface the scheduled resume time in the smartgrid timestamp area.
        const sgTimestamp = elements['smartgrid-timestamp'];
        if (sgTimestamp) {
            if (data.scheduled_resume_at) {
                const t = new Date(data.scheduled_resume_at);
                sgTimestamp.textContent = `auto-resume ${t.toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'})}`;
            } else if (!data.powersave) {
                sgTimestamp.textContent = '';
            }
        }
    } catch (err) {
        console.error('Error toggling powersave:', err);
        toggleEl.classList.add('error');
        setTimeout(() => toggleEl.classList.remove('error'), 600);
    } finally {
        state.powersaveToggling = false;
        toggleEl.classList.remove('loading');
    }
}

// Price level colors (green=very_cheap → red=very_expensive)
const PRICE_LEVEL_COLORS = {
    very_cheap: '#4ade80',    // Bright green
    cheap: '#22c55e',         // Green
    normal: '#f59e0b',        // Amber
    expensive: '#f87171',     // Light red
    very_expensive: '#ef4444' // Red
};

// Format price value
function formatPrice(value, decimals = 2) {
    if (value === null || value === undefined || isNaN(value)) return '--';
    return value.toFixed(decimals);
}

// Format time from ISO8601 string
function formatPriceTime(isoString) {
    if (!isoString) return '--:--';
    const date = new Date(isoString);
    return date.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

// Get color for price level
function getPriceLevelColor(level) {
    return PRICE_LEVEL_COLORS[level] || '#8b8b9e';
}

// Draw price chart on canvas
function drawPriceChart(canvas, prices, currentTime) {
    if (!canvas || !prices || prices.length === 0) return;

    const ctx = canvas.getContext('2d');
    const dpr = window.devicePixelRatio || 1;

    // Set canvas size accounting for device pixel ratio
    const rect = canvas.getBoundingClientRect();
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);

    const width = rect.width;
    const height = rect.height;
    const padding = { top: 20, right: 15, bottom: 25, left: 40 };
    const chartWidth = width - padding.left - padding.right;
    const chartHeight = height - padding.top - padding.bottom;

    // Clear canvas
    ctx.clearRect(0, 0, width, height);

    // Find min/max prices for scaling
    const priceValues = prices.map(p => p.spot_sek);
    const minPrice = Math.min(...priceValues);
    const maxPrice = Math.max(...priceValues);
    const priceRange = maxPrice - minPrice || 1;

    // Calculate positions
    const pointWidth = chartWidth / prices.length;

    // Draw grid lines and labels
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.1)';
    ctx.fillStyle = '#8b8b9e';
    ctx.font = '10px system-ui';
    ctx.textAlign = 'right';

    const gridLines = 4;
    for (let i = 0; i <= gridLines; i++) {
        const y = padding.top + (chartHeight * i / gridLines);
        const price = maxPrice - (priceRange * i / gridLines);

        ctx.beginPath();
        ctx.moveTo(padding.left, y);
        ctx.lineTo(width - padding.right, y);
        ctx.stroke();

        ctx.fillText(price.toFixed(2), padding.left - 5, y + 3);
    }

    // Draw colored line segments
    ctx.lineWidth = 2;
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';

    // Draw filled area under the line
    ctx.beginPath();
    ctx.moveTo(padding.left, padding.top + chartHeight);

    for (let i = 0; i < prices.length; i++) {
        const x = padding.left + i * pointWidth + pointWidth / 2;
        const y = padding.top + chartHeight - ((prices[i].spot_sek - minPrice) / priceRange * chartHeight);
        if (i === 0) {
            ctx.lineTo(x, y);
        } else {
            ctx.lineTo(x, y);
        }
    }

    ctx.lineTo(padding.left + prices.length * pointWidth, padding.top + chartHeight);
    ctx.closePath();
    ctx.fillStyle = 'rgba(59, 130, 246, 0.1)';
    ctx.fill();

    // Draw colored line segments
    for (let i = 0; i < prices.length - 1; i++) {
        const x1 = padding.left + i * pointWidth + pointWidth / 2;
        const y1 = padding.top + chartHeight - ((prices[i].spot_sek - minPrice) / priceRange * chartHeight);
        const x2 = padding.left + (i + 1) * pointWidth + pointWidth / 2;
        const y2 = padding.top + chartHeight - ((prices[i + 1].spot_sek - minPrice) / priceRange * chartHeight);

        const level = prices[i].level || 'normal';
        ctx.strokeStyle = getPriceLevelColor(level);

        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
    }

    // Draw current time marker
    if (currentTime) {
        const now = new Date();
        for (let i = 0; i < prices.length; i++) {
            const start = new Date(prices[i].starts_at);
            const end = new Date(prices[i].ends_at);
            if (now >= start && now < end) {
                const x = padding.left + i * pointWidth + pointWidth / 2;

                // Vertical line
                ctx.strokeStyle = 'rgba(255, 255, 255, 0.5)';
                ctx.setLineDash([3, 3]);
                ctx.beginPath();
                ctx.moveTo(x, padding.top);
                ctx.lineTo(x, padding.top + chartHeight);
                ctx.stroke();
                ctx.setLineDash([]);

                // Now marker
                ctx.fillStyle = '#fff';
                ctx.font = 'bold 9px system-ui';
                ctx.textAlign = 'center';
                ctx.fillText('NOW', x, padding.top - 5);

                break;
            }
        }
    }

    // Draw time labels (every 4 hours for 24h data)
    ctx.fillStyle = '#8b8b9e';
    ctx.font = '9px system-ui';
    ctx.textAlign = 'center';

    const hourInterval = prices.length <= 24 ? 4 : 16; // 4h for hourly, 4h for 15-min
    for (let i = 0; i < prices.length; i += hourInterval) {
        const x = padding.left + i * pointWidth + pointWidth / 2;
        const time = formatPriceTime(prices[i].starts_at);
        ctx.fillText(time, x, height - 5);
    }

    // Store chart state for mouse interaction
    chartState.prices = prices;
    chartState.padding = padding;
    chartState.pointWidth = pointWidth;
    chartState.chartWidth = chartWidth;
    chartState.chartHeight = chartHeight;
    chartState.minPrice = minPrice;
    chartState.priceRange = priceRange;

    // Draw crosshair if hovering
    if (chartState.hoverIndex !== null && chartState.hoverIndex >= 0 && chartState.hoverIndex < prices.length) {
        const hoverPrice = prices[chartState.hoverIndex];
        const hx = padding.left + chartState.hoverIndex * pointWidth + pointWidth / 2;
        const hy = padding.top + chartHeight - ((hoverPrice.spot_sek - minPrice) / priceRange * chartHeight);

        // Dashed vertical line
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.4)';
        ctx.setLineDash([3, 3]);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(hx, padding.top);
        ctx.lineTo(hx, padding.top + chartHeight);
        ctx.stroke();

        // Dashed horizontal line
        ctx.beginPath();
        ctx.moveTo(padding.left, hy);
        ctx.lineTo(padding.left + chartWidth, hy);
        ctx.stroke();
        ctx.setLineDash([]);

        // Circle at intersection (outer white)
        ctx.fillStyle = '#fff';
        ctx.beginPath();
        ctx.arc(hx, hy, 6, 0, Math.PI * 2);
        ctx.fill();

        // Inner circle with price level color
        const levelColor = getPriceLevelColor(hoverPrice.level);
        ctx.fillStyle = levelColor;
        ctx.beginPath();
        ctx.arc(hx, hy, 4, 0, Math.PI * 2);
        ctx.fill();
    }
}

// Fetch and update electricity prices
async function updatePrices() {
    try {
        const data = await fetchJson(`${API_BASE}/prices`);

        // Update current price display
        if (data.current) {
            const current = data.current;

            // Time period
            const timeEl = elements['price-time'];
            if (timeEl) {
                const startTime = formatPriceTime(current.starts_at);
                const endTime = formatPriceTime(current.ends_at);
                timeEl.textContent = `${startTime} - ${endTime}`;
            }

            // Main spot price
            const spotEl = elements['price-spot'];
            if (spotEl) {
                spotEl.textContent = formatPrice(current.spot_sek);
            }

            // Price level badge
            const levelEl = elements['price-level'];
            if (levelEl) {
                const level = current.level || 'normal';
                levelEl.textContent = level.replace('_', ' ');
                levelEl.className = 'price-level-badge ' + level;
            }

            // Spot detail
            const spotDetailEl = elements['price-spot-detail'];
            if (spotDetailEl) {
                spotDetailEl.textContent = `${formatPrice(current.spot_sek)} kr/kWh`;
            }

            // Tibber section (show if available)
            const tibberRows = document.querySelectorAll('.tibber-row');
            if (data.tibber_available && current.tibber_total !== null) {
                tibberRows.forEach(row => row.style.display = 'flex');

                const tibberTotalEl = elements['price-tibber-total'];
                if (tibberTotalEl) {
                    tibberTotalEl.textContent = `${formatPrice(current.tibber_total)} kr/kWh`;
                }

                const markupEl = elements['price-markup'];
                if (markupEl && current.markup !== null) {
                    const sign = current.markup >= 0 ? '+' : '';
                    const pct = current.markup_percent ? ` (${sign}${current.markup_percent.toFixed(0)}%)` : '';
                    markupEl.textContent = `${sign}${formatPrice(current.markup)} kr${pct}`;
                }
            } else {
                tibberRows.forEach(row => row.style.display = 'none');
            }
        }

        // Update statistics (today)
        if (data.today && data.today.spot_statistics) {
            const stats = data.today.spot_statistics;
            const minEl = elements['price-min'];
            const maxEl = elements['price-max'];
            const avgEl = elements['price-avg'];

            if (minEl) minEl.textContent = formatPrice(stats.min);
            if (maxEl) maxEl.textContent = formatPrice(stats.max);
            if (avgEl) avgEl.textContent = formatPrice(stats.mean);
        }

        // Draw price chart
        const chartCanvas = elements['price-chart'];
        if (chartCanvas && data.today && data.today.prices && data.today.prices.length > 0) {
            // Combine today and tomorrow prices for the chart
            let chartPrices = [...data.today.prices];
            if (data.tomorrow && data.tomorrow.available && data.tomorrow.prices) {
                chartPrices = chartPrices.concat(data.tomorrow.prices);
            }
            drawPriceChart(chartCanvas, chartPrices, true);
        }
    } catch (err) {
        console.error('Error fetching prices:', err);
    }
}

// Fetch and update heat pump statistics
async function updateHeatPumpStats() {
    try {
        const data = await fetchJson(`${API_BASE}/heatpump/stats`);

        // Cycle statistics
        if (data.cycle_stats) {
            const el = (id) => elements[id];
            el('cycle-min').textContent = formatDuration(data.cycle_stats.min_secs);
            el('cycle-max').textContent = formatDuration(data.cycle_stats.max_secs);
            el('cycle-avg').textContent = formatDuration(Math.round(data.cycle_stats.avg_secs));
            el('cycle-count').textContent = data.cycle_stats.cycle_count;
        } else {
            elements['cycle-min'].textContent = '--:--';
            elements['cycle-max'].textContent = '--:--';
            elements['cycle-avg'].textContent = '--:--';
            elements['cycle-count'].textContent = '0';
        }

        // Starts per window
        if (data.starts) {
            elements['starts-hour'].textContent = data.starts.this_hour;
            elements['starts-day'].textContent = data.starts.this_day;
            elements['starts-week'].textContent = data.starts.this_week;
            elements['starts-month'].textContent = data.starts.this_month;
            elements['starts-year'].textContent = data.starts.this_year;
        }

        // Operating hours per window
        if (data.operating_hours) {
            const oh = data.operating_hours;
            elements['ophours-hour'].textContent = `${(oh.this_hour * 60).toFixed(0)} min`;
            elements['ophours-day'].textContent = `${oh.this_day.toFixed(1)} h`;
            elements['ophours-week'].textContent = `${oh.this_week.toFixed(1)} h`;
            elements['ophours-month'].textContent = `${oh.this_month.toFixed(1)} h`;
            elements['ophours-year'].textContent = `${oh.this_year.toFixed(1)} h`;
        }

        // Tracking info
        if (data.tracking) {
            const trackingEl = elements['hp-tracking-info'];
            if (trackingEl) {
                const badge = trackingEl.querySelector('.tracking-badge');
                if (badge) {
                    const hours = data.tracking.tracking_hours.toFixed(1);
                    badge.textContent = `Tracking: ${hours}h | ${data.tracking.total_starts} starts`;
                }
            }
        }
    } catch (err) {
        console.error('Error fetching heat pump stats:', err);
    }
}

// Show heat pump stats modal with charts
async function showStatsModal() {
    const modal = elements['stats-modal'];
    if (!modal) return;

    modal.style.display = 'flex';

    // Fetch history data
    try {
        hpStatsState.historyData = await fetchJson(`${API_BASE}/heatpump/stats/history?days=30`);
        renderStatsChart(hpStatsState.currentChartType);
    } catch (err) {
        console.error('Error fetching stats history:', err);
    }
}

// Hide stats modal
function hideStatsModal() {
    const modal = elements['stats-modal'];
    if (modal) {
        modal.style.display = 'none';
    }
    if (hpStatsState.chart) {
        hpStatsState.chart.destroy();
        hpStatsState.chart = null;
    }
}

// Render stats chart based on type
function renderStatsChart(chartType) {
    const canvas = elements['stats-chart'];
    const emptyState = elements['chart-empty'];
    if (!canvas || !hpStatsState.historyData) return;

    // Destroy existing chart
    if (hpStatsState.chart) {
        hpStatsState.chart.destroy();
        hpStatsState.chart = null;
    }

    const data = hpStatsState.historyData;
    let chartConfig;
    let hasData = false;

    switch (chartType) {
        case 'cycles':
            hasData = data.cycles && data.cycles.length > 0;
            if (hasData) chartConfig = buildCyclesChart(data.cycles);
            break;
        case 'hours':
            hasData = data.daily && data.daily.length > 0;
            if (hasData) chartConfig = buildDailyHoursChart(data.daily);
            break;
        case 'starts':
            hasData = data.daily && data.daily.length > 0;
            if (hasData) chartConfig = buildDailyStartsChart(data.daily);
            break;
        case 'cycle-temp':
            hasData = data.cycles && data.cycles.some(c => c.outdoor_temp_c !== null);
            if (hasData) chartConfig = buildCycleTempChart(data.cycles);
            break;
        case 'hours-temp':
            hasData = data.daily && data.daily.some(d => d.avg_outdoor_temp_c !== null);
            if (hasData) chartConfig = buildHoursTempChart(data.daily);
            break;
        default:
            hasData = data.cycles && data.cycles.length > 0;
            if (hasData) chartConfig = buildCyclesChart(data.cycles);
    }

    // Show/hide empty state
    if (emptyState) {
        emptyState.style.display = hasData ? 'none' : 'flex';
    }
    canvas.style.display = hasData ? 'block' : 'none';

    if (hasData && chartConfig) {
        hpStatsState.chart = new Chart(canvas, chartConfig);
    }

    hpStatsState.currentChartType = chartType;
}

// Build cycles bar chart (cycle durations over time)
function buildCyclesChart(cycles) {
    const labels = cycles.map(c => {
        const d = new Date(c.timestamp);
        return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    });
    const durations = cycles.map(c => c.duration_secs / 60); // Convert to minutes

    return {
        type: 'bar',
        data: {
            labels,
            datasets: [{
                label: 'Cycle Duration (min)',
                data: durations,
                backgroundColor: 'rgba(59, 130, 246, 0.7)',
                borderColor: 'rgba(59, 130, 246, 1)',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                title: { display: true, text: 'Recent Cycle Durations', color: '#fff' },
                legend: { labels: { color: '#fff' } }
            },
            scales: {
                x: { ticks: { color: '#888', maxRotation: 45 }, grid: { color: 'rgba(255,255,255,0.1)' } },
                y: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Minutes', color: '#888' } }
            }
        }
    };
}

// Build daily operating hours bar chart
function buildDailyHoursChart(daily) {
    const labels = daily.map(d => d.date.slice(5)); // MM-DD
    const hours = daily.map(d => d.operating_hours);

    return {
        type: 'bar',
        data: {
            labels,
            datasets: [{
                label: 'Operating Hours',
                data: hours,
                backgroundColor: 'rgba(34, 197, 94, 0.7)',
                borderColor: 'rgba(34, 197, 94, 1)',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                title: { display: true, text: 'Daily Operating Hours', color: '#fff' },
                legend: { labels: { color: '#fff' } }
            },
            scales: {
                x: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' } },
                y: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Hours', color: '#888' } }
            }
        }
    };
}

// Build daily starts bar chart
function buildDailyStartsChart(daily) {
    const labels = daily.map(d => d.date.slice(5)); // MM-DD
    const starts = daily.map(d => d.starts);

    return {
        type: 'bar',
        data: {
            labels,
            datasets: [{
                label: 'Compressor Starts',
                data: starts,
                backgroundColor: 'rgba(245, 158, 11, 0.7)',
                borderColor: 'rgba(245, 158, 11, 1)',
                borderWidth: 1
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                title: { display: true, text: 'Daily Compressor Starts', color: '#fff' },
                legend: { labels: { color: '#fff' } }
            },
            scales: {
                x: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' } },
                y: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Starts', color: '#888' } }
            }
        }
    };
}

// Build cycle time vs outdoor temp scatter chart
function buildCycleTempChart(cycles) {
    const points = cycles
        .filter(c => c.outdoor_temp_c !== null)
        .map(c => ({ x: c.outdoor_temp_c, y: c.duration_secs / 60 }));

    return {
        type: 'scatter',
        data: {
            datasets: [{
                label: 'Cycle Duration vs Outdoor Temp',
                data: points,
                backgroundColor: 'rgba(139, 92, 246, 0.7)',
                borderColor: 'rgba(139, 92, 246, 1)',
                pointRadius: 5
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                title: { display: true, text: 'Cycle Duration vs Outdoor Temperature', color: '#fff' },
                legend: { labels: { color: '#fff' } }
            },
            scales: {
                x: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Outdoor Temp (°C)', color: '#888' } },
                y: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Duration (min)', color: '#888' } }
            }
        }
    };
}

// Build daily hours vs outdoor temp scatter chart
function buildHoursTempChart(daily) {
    const points = daily
        .filter(d => d.avg_outdoor_temp_c !== null)
        .map(d => ({ x: d.avg_outdoor_temp_c, y: d.operating_hours }));

    return {
        type: 'scatter',
        data: {
            datasets: [{
                label: 'Daily Hours vs Avg Outdoor Temp',
                data: points,
                backgroundColor: 'rgba(236, 72, 153, 0.7)',
                borderColor: 'rgba(236, 72, 153, 1)',
                pointRadius: 5
            }]
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: {
                title: { display: true, text: 'Daily Operating Hours vs Avg Outdoor Temperature', color: '#fff' },
                legend: { labels: { color: '#fff' } }
            },
            scales: {
                x: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Avg Outdoor Temp (°C)', color: '#888' } },
                y: { ticks: { color: '#888' }, grid: { color: 'rgba(255,255,255,0.1)' }, title: { display: true, text: 'Hours', color: '#888' } }
            }
        }
    };
}

// Main update function
async function updateAll() {
    try {
        // Run all updates in parallel
        await Promise.all([
            updateTemperatures(),
            updateTankTemps(),
            updateHeatingSystem(),
            updateHeatPump(),
            updateAlarms(),
            updateSystemStatus(),
            updateGrid(),
            updatePrices(),
            updateHeatPumpStats()
        ]);

        setConnectionStatus('connected');
        updateTimestamp();
    } catch (err) {
        console.error('Update failed:', err);
        setConnectionStatus('error');
    }
}

// Initialize and start updates
function init() {
    initElements();

    // Powersave toggle handler
    const powersaveToggle = elements['powersave-toggle'];
    if (powersaveToggle) {
        powersaveToggle.addEventListener('click', togglePowersave);
        powersaveToggle.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                togglePowersave();
            }
        });
    }

    // Peak popup click/tap support (for mobile where hover doesn't work)
    const peakStat = elements['peak-stat'];
    if (peakStat) {
        peakStat.addEventListener('click', (e) => {
            e.stopPropagation();
            const popup = elements['peak-popup'];
            if (popup) {
                const isVisible = popup.style.opacity === '1';
                popup.style.opacity = isVisible ? '0' : '1';
                popup.style.visibility = isVisible ? 'hidden' : 'visible';
            }
        });

        // Close popup when clicking outside
        document.addEventListener('click', (e) => {
            if (!peakStat.contains(e.target)) {
                const popup = elements['peak-popup'];
                if (popup) {
                    popup.style.opacity = '0';
                    popup.style.visibility = 'hidden';
                }
            }
        });
    }

    // Price chart hover/touch interaction
    const chartCanvas = elements['price-chart'];
    const chartTooltip = elements['chart-tooltip'];

    if (chartCanvas && chartTooltip) {
        // Shared function to handle chart interaction (mouse or touch)
        function handleChartInteraction(clientX, clientY) {
            if (chartState.prices.length === 0 || !chartState.padding || !chartState.pointWidth) {
                return;
            }

            const rect = chartCanvas.getBoundingClientRect();
            const scaleX = chartCanvas.width / rect.width;
            const x = (clientX - rect.left) * scaleX / (window.devicePixelRatio || 1);

            // Calculate which price bar the pointer is over
            const index = Math.floor((x - chartState.padding.left) / chartState.pointWidth);

            if (index >= 0 && index < chartState.prices.length) {
                chartState.hoverIndex = index;
                const price = chartState.prices[index];

                // Update tooltip content
                const time = formatPriceTime(price.starts_at);
                chartTooltip.textContent = `${time}: ${price.spot_sek.toFixed(2)} kr/kWh`;

                // Position tooltip above the point
                const pointX = chartState.padding.left + index * chartState.pointWidth + chartState.pointWidth / 2;
                const tooltipX = (pointX / scaleX) * (window.devicePixelRatio || 1);

                chartTooltip.style.display = 'block';
                chartTooltip.style.left = `${tooltipX / (window.devicePixelRatio || 1)}px`;
                chartTooltip.style.top = '5px';

                // Redraw chart with crosshair
                drawPriceChart(chartCanvas, chartState.prices, true);
            } else {
                clearChartInteraction();
            }
        }

        // Clear the chart interaction state
        function clearChartInteraction() {
            chartState.hoverIndex = null;
            chartTooltip.style.display = 'none';
            if (chartState.prices.length > 0) {
                drawPriceChart(chartCanvas, chartState.prices, true);
            }
        }

        // Mouse events
        chartCanvas.addEventListener('mousemove', (e) => {
            handleChartInteraction(e.clientX, e.clientY);
        });

        chartCanvas.addEventListener('mouseleave', () => {
            clearChartInteraction();
        });

        // Touch events for mobile
        chartCanvas.addEventListener('touchstart', (e) => {
            if (e.touches.length === 1) {
                e.preventDefault(); // Prevent scrolling while interacting with chart
                const touch = e.touches[0];
                handleChartInteraction(touch.clientX, touch.clientY);
            }
        }, { passive: false });

        chartCanvas.addEventListener('touchmove', (e) => {
            if (e.touches.length === 1) {
                e.preventDefault();
                const touch = e.touches[0];
                handleChartInteraction(touch.clientX, touch.clientY);
            }
        }, { passive: false });

        chartCanvas.addEventListener('touchend', () => {
            clearChartInteraction();
        });

        chartCanvas.addEventListener('touchcancel', () => {
            clearChartInteraction();
        });
    }

    // Heat pump stats panel click handler (opens modal)
    const statsPanel = elements['heatpump-stats-panel'];
    if (statsPanel) {
        statsPanel.addEventListener('click', showStatsModal);
    }

    // Stats modal close button
    const modalClose = elements['stats-modal-close'];
    if (modalClose) {
        modalClose.addEventListener('click', hideStatsModal);
    }

    // Stats modal background click to close
    const statsModal = elements['stats-modal'];
    if (statsModal) {
        statsModal.addEventListener('click', (e) => {
            // Only close if clicking on the overlay background, not the content
            if (e.target === statsModal) {
                hideStatsModal();
            }
        });
    }

    // Escape key to close modal
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && statsModal && statsModal.style.display !== 'none') {
            hideStatsModal();
        }
    });

    // Stats modal tab buttons
    const tabButtons = document.querySelectorAll('.tab-btn');
    tabButtons.forEach(btn => {
        btn.addEventListener('click', () => {
            // Update active tab styling
            tabButtons.forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            // Render the selected chart type
            const chartType = btn.dataset.chart;
            renderStatsChart(chartType);
        });
    });

    updateAll();
    setInterval(updateAll, REFRESH_INTERVAL);
}

// Start when DOM is ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
} else {
    init();
}
