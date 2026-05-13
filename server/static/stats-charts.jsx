/* Stats charts — histogram, 24×7 heatmap, calendar heatmap, monthly stacked, radial clock.
   Data sources are wired via props from buildStatsData() in hooks.jsx; nothing
   here synthesizes data. When data is missing each chart renders an empty
   placeholder. */

const EmptyChart = ({ label = "No data yet" }) => (
  <div className="empty-chart">{label}</div>
);

/* ---------- Histogram (cycle duration buckets) ---------- */
const CycleHistogram = ({ data }) => {
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const buckets = [
    { label: "0-5",  range: [0, 5],  tone: "warn" },
    { label: "5-10", range: [5, 10], tone: "ok" },
    { label: "10-20",range: [10, 20],tone: "ok" },
    { label: "20-40",range: [20, 40],tone: "ok" },
    { label: "40+",  range: [40, 99],tone: "neutral" },
  ];
  const counts = buckets.map(b => data.filter(d => d >= b.range[0] && d < b.range[1]).length);
  const max = Math.max(...counts, 1);
  const total = data.length;

  return (
    <div className="hist">
      <div className="hist-bars">
        {buckets.map((b, i) => (
          <div key={i} className={`hist-bar tone-${b.tone}`}>
            <div className="bar-wrap">
              <span className="count">{counts[i]}</span>
              <div className="bar" style={{ height: `${(counts[i] / max) * 100}%` }}/>
            </div>
            <div className="lbl">{b.label}<span className="u">min</span></div>
          </div>
        ))}
      </div>
      <div className="hist-foot">
        <span><strong>{counts[0]}</strong> short cycles ({((counts[0]/total)*100).toFixed(1)}%) · <strong>{total}</strong> total</span>
        <span style={{ color: "var(--text-3)" }}>cycles &lt; 5 min suggest short-cycling</span>
      </div>
    </div>
  );
};

/* ---------- 24×7 heatmap ---------- */
const HourHeatmap = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const flat = data.flat();
  if (flat.every(v => v === 0)) return <EmptyChart/>;
  const min = Math.min(...flat), max = Math.max(...flat);
  const dayLbl = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"];

  const colorFor = v => {
    const t = (v - min) / (max - min);
    return `oklch(${0.30 + t * 0.40} ${0.04 + t * 0.10} 220)`;
  };

  return (
    <div className="hourheat">
      <div className="hh-grid">
        <div className="hh-corner"/>
        {Array.from({length: 24}, (_, h) => (
          <div key={h} className="hh-col-lbl">{h % 4 === 0 ? `${h.toString().padStart(2,"0")}` : ""}</div>
        ))}
        {data.map((row, dow) => (
          <React.Fragment key={dow}>
            <div className="hh-row-lbl">{dayLbl[dow]}</div>
            {row.map((v, h) => (
              <div key={h}
                   className={`hh-cell ${hover && hover.dow===dow && hover.h===h ? "hover" : ""}`}
                   style={{ background: colorFor(v) }}
                   onMouseEnter={() => setHover({ dow, h, v })}
                   onMouseLeave={() => setHover(null)}/>
            ))}
          </React.Fragment>
        ))}
      </div>
      <div className="hh-foot">
        <div className="hh-legend">
          <span>shorter</span>
          <span className="hh-grad"/>
          <span>longer</span>
        </div>
        <div className="hh-tip">
          {hover
            ? `${dayLbl[hover.dow]} ${hover.h.toString().padStart(2,"0")}:00 — avg ${hover.v.toFixed(1)} min`
            : "hover a cell"}
        </div>
      </div>
    </div>
  );
};

/* ---------- Calendar heatmap (GitHub-style) ---------- */
const CalendarHeatmap = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  // pad start so first column begins on Monday
  const first = data[0].date;
  const padStart = (first.getDay() + 6) % 7; // mon=0
  const cells = Array(padStart).fill(null).concat(data);
  const cols = Math.ceil(cells.length / 7);
  const max = Math.max(...data.map(d => d.hours));

  const colorFor = h => {
    if (h == null) return "transparent";
    if (h < 0.5) return "color-mix(in oklch, var(--text-3) 14%, transparent)";
    const t = Math.min(1, h / max);
    return `oklch(${0.42 + t * 0.32} ${0.06 + t * 0.10} 200)`;
  };

  // month labels at first column of each month
  const monthLabels = [];
  let lastMonth = -1;
  for (let c = 0; c < cols; c++) {
    const cell = cells[c * 7];
    if (cell && cell.date && cell.date.getMonth() !== lastMonth) {
      monthLabels.push({ col: c, label: cell.date.toLocaleDateString("en-GB", { month: "short" }) });
      lastMonth = cell.date.getMonth();
    }
  }

  return (
    <div className="calheat">
      <div className="ch-months" style={{ gridTemplateColumns: `repeat(${cols}, 12px)` }}>
        {Array.from({length: cols}).map((_, c) => {
          const m = monthLabels.find(x => x.col === c);
          return <div key={c} className="ch-mlbl">{m ? m.label : ""}</div>;
        })}
      </div>
      <div className="ch-grid" style={{ gridTemplateColumns: `repeat(${cols}, 12px)` }}>
        {Array.from({length: cols * 7}).map((_, idx) => {
          const col = Math.floor(idx / 7);
          const row = idx % 7;
          const cellIdx = col * 7 + row;
          const cell = cells[cellIdx];
          if (!cell) return <div key={idx} className="ch-cell empty"/>;
          return (
            <div key={idx}
                 className={`ch-cell ${hover === cellIdx ? "hover" : ""}`}
                 style={{ background: colorFor(cell.hours), gridRow: row + 1, gridColumn: col + 1 }}
                 onMouseEnter={() => setHover(cellIdx)}
                 onMouseLeave={() => setHover(null)}/>
          );
        })}
      </div>
      <div className="ch-foot">
        <div className="ch-tip">
          {hover != null && cells[hover]
            ? `${cells[hover].date.toLocaleDateString("en-GB", { day:"numeric", month:"short", year:"numeric" })} — ${cells[hover].hours.toFixed(1)} h`
            : "hover a day"}
        </div>
        <div className="ch-legend">
          <span>0 h</span>
          {[0.1, 0.3, 0.6, 1.0].map((t, i) => (
            <span key={i} className="ch-leg-cell" style={{ background: colorFor(t * max) }}/>
          ))}
          <span>{max.toFixed(0)} h</span>
        </div>
      </div>
    </div>
  );
};

/* ---------- Monthly stacked bars ---------- */
const MonthlyStacked = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const totals = data.map(m => m.heat + m.dhw);
  const max = Math.max(...totals, 1);

  return (
    <div className="monthly">
      <div className="m-bars">
        {data.map((m, i) => {
          const total = m.heat + m.dhw;
          const heatH = (m.heat / max) * 100;
          const dhwH  = (m.dhw  / max) * 100;
          return (
            <div key={i}
                 className={`m-bar ${hover === i ? "hover" : ""}`}
                 onMouseEnter={() => setHover(i)}
                 onMouseLeave={() => setHover(null)}>
              <div className="m-stack">
                <div className="m-seg dhw"  style={{ height: `${dhwH}%`  }}/>
                <div className="m-seg heat" style={{ height: `${heatH}%` }}/>
              </div>
              <div className="m-lbl">{m.label}</div>
            </div>
          );
        })}
      </div>
      <div className="m-foot">
        <div className="m-legend">
          <span><span className="sw heat"/>Heating</span>
          <span><span className="sw dhw"/>Hot water</span>
        </div>
        <div className="m-tip">
          {hover != null
            ? `${data[hover].label} ${data[hover].year} — ${data[hover].heat} h heat · ${data[hover].dhw} h DHW · ${(data[hover].heat + data[hover].dhw)} h total`
            : "hover a bar"}
        </div>
      </div>
    </div>
  );
};

/* ---------- Radial 24h clock ---------- */
const RadialClock = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const cx = 150, cy = 150, rIn = 55, rOut = 130;

  const wedges = data.map((v, h) => {
    const a0 = (h / 24) * Math.PI * 2 - Math.PI / 2;
    const a1 = ((h + 1) / 24) * Math.PI * 2 - Math.PI / 2;
    const r  = rIn + v * (rOut - rIn);
    const x0i = cx + Math.cos(a0) * rIn;
    const y0i = cy + Math.sin(a0) * rIn;
    const x1i = cx + Math.cos(a1) * rIn;
    const y1i = cy + Math.sin(a1) * rIn;
    const x0o = cx + Math.cos(a0) * r;
    const y0o = cy + Math.sin(a0) * r;
    const x1o = cx + Math.cos(a1) * r;
    const y1o = cy + Math.sin(a1) * r;
    const path = `M ${x0i} ${y0i} L ${x0o} ${y0o} A ${r} ${r} 0 0 1 ${x1o} ${y1o} L ${x1i} ${y1i} A ${rIn} ${rIn} 0 0 0 ${x0i} ${y0i} Z`;
    const t = v;
    const fill = `oklch(${0.45 + t * 0.30} ${0.06 + t * 0.12} 200)`;
    return { h, v, path, fill };
  });

  return (
    <div className="radial">
      <svg viewBox="0 0 300 300" className="radial-svg">
        {/* outer ring */}
        <circle cx={cx} cy={cy} r={rOut} fill="none" stroke="var(--line)" strokeWidth="1"/>
        <circle cx={cx} cy={cy} r={rIn}  fill="none" stroke="var(--line)" strokeWidth="1"/>
        {wedges.map(w => (
          <path key={w.h} d={w.path} fill={w.fill}
                stroke={hover === w.h ? "var(--text)" : "transparent"}
                strokeWidth="1.5"
                onMouseEnter={() => setHover(w.h)}
                onMouseLeave={() => setHover(null)}/>
        ))}
        {/* hour ticks */}
        {[0, 6, 12, 18].map(h => {
          const a = (h / 24) * Math.PI * 2 - Math.PI / 2;
          const x = cx + Math.cos(a) * (rOut + 14);
          const y = cy + Math.sin(a) * (rOut + 14);
          return (
            <text key={h} x={x} y={y} textAnchor="middle" dominantBaseline="middle"
                  fontSize="11" fill="var(--text-3)" fontFamily="var(--font-mono)">
              {h.toString().padStart(2,"0")}
            </text>
          );
        })}
        <text x={cx} y={cy - 6} textAnchor="middle" fontSize="11" fill="var(--text-3)">avg duty</text>
        <text x={cx} y={cy + 12} textAnchor="middle" fontSize="18" fill="var(--text)"
              fontFamily="var(--font-mono)" fontWeight="600">
          {hover != null ? `${(data[hover] * 100).toFixed(0)}%` : `${(data.reduce((a,b)=>a+b,0)/24*100).toFixed(0)}%`}
        </text>
      </svg>
      <div className="radial-tip">
        {hover != null
          ? `${hover.toString().padStart(2,"0")}:00 — runs ~${(data[hover] * 60).toFixed(0)} min/h on average`
          : "Hover a wedge to inspect the average runtime for that hour. Two daily peaks are typical: morning DHW draws and evening heating."}
      </div>
    </div>
  );
};

/* ---------- Heating system: flow/return/ΔT trend + step response ---------- */
const HeatingTrend = ({ data, height = 220 }) => {
  const [hov, setHov] = React.useState(null);
  if (!data || !Array.isArray(data.flow) || !Array.isArray(data.ret)
      || data.flow.length === 0 || data.ret.length === 0) {
    return <EmptyChart/>;
  }
  const w = 1200, h = height, pad = 36;
  // Label each bucket with the wall-clock hour at its end so the rightmost
  // tick reads the current local hour.
  const nowHour = new Date().getHours();
  const labels = Array.from({ length: 24 }, (_, i) =>
    `${((nowHour + 1 + i) % 24).toString().padStart(2,"0")}:00`);
  // Filter nulls (gap hours where the sensor was offline or cache missed)
  // out of axis computation; the chart paths render those buckets as gaps.
  const finite = [...data.flow, ...data.ret].filter(v => v != null);
  const yMin = finite.length ? Math.floor(Math.min(...finite) - 1) : 0;
  const flowFinite = data.flow.filter(v => v != null);
  const yMax = flowFinite.length ? Math.ceil(Math.max(...flowFinite) + 1) : yMin + 1;
  const x = i => pad + (i / 23) * (w - pad * 1.5);
  const y = v => h - pad - ((v - yMin) / (yMax - yMin)) * (h - pad * 1.5);
  // Path for a series that may contain nulls. Each null breaks the line
  // so gaps stay visibly empty instead of zero-diving.
  const seriesPath = (arr) => {
    let started = false;
    return arr
      .map((v, i) => {
        if (v == null) { started = false; return ""; }
        const cmd = started ? "L" : "M";
        started = true;
        return `${cmd} ${x(i)} ${y(v)}`;
      })
      .join(" ");
  };
  const flowPath = seriesPath(data.flow);
  const retPath = seriesPath(data.ret);
  // ΔT band — close a polygon over each contiguous run where both flow
  // and ret are non-null. Gaps produce separate polygons.
  const bandPath = (() => {
    const segments = [];
    let run = [];
    const flush = () => {
      if (run.length < 2) { run = []; return; }
      const top = run.map(i => `L ${x(i)} ${y(data.flow[i])}`).join(" ");
      const bottom = run.slice().reverse().map(i => `L ${x(i)} ${y(data.ret[i])}`).join(" ");
      segments.push(`M ${x(run[0])} ${y(data.flow[run[0]])} ${top.slice(1)} ${bottom} Z`);
      run = [];
    };
    for (let i = 0; i < data.flow.length; i++) {
      if (data.flow[i] != null && data.ret[i] != null) run.push(i);
      else flush();
    }
    flush();
    return segments.join(" ");
  })();
  // Average ΔT — only across hours where both series have data.
  const avgDelta = (() => {
    let sum = 0, n = 0;
    for (let i = 0; i < data.flow.length; i++) {
      if (data.flow[i] != null && data.ret[i] != null) {
        sum += data.flow[i] - data.ret[i];
        n += 1;
      }
    }
    return n > 0 ? sum / n : null;
  })();
  const formatDeg = (v) => (v == null ? "—" : `${v.toFixed(1)}°`);
  return (
    <div className="heating-trend">
      <svg viewBox={`0 0 ${w} ${h}`} className="ht-svg" onMouseLeave={() => setHov(null)}
           onMouseMove={e => {
             const r = e.currentTarget.getBoundingClientRect();
             const px = ((e.clientX - r.left) / r.width) * w;
             const i = Math.round(((px - pad) / (w - pad * 1.5)) * 23);
             setHov(Math.max(0, Math.min(23, i)));
           }}>
        {/* gridlines */}
        {Array.from({length: 5}).map((_, j) => {
          const v = yMin + ((yMax - yMin) / 4) * j;
          return (
            <g key={j}>
              <line x1={pad} y1={y(v)} x2={w} y2={y(v)} stroke="var(--line)" strokeDasharray="2 4" opacity="0.5"/>
              <text x={pad - 6} y={y(v) + 3} textAnchor="end" fontSize="10" fill="var(--text-3)" fontFamily="var(--font-mono)">{v.toFixed(0)}°</text>
            </g>
          );
        })}
        {[0,6,12,18,23].map(i => (
          <text key={i} x={x(i)} y={h - pad + 14} textAnchor="middle" fontSize="10" fill="var(--text-3)" fontFamily="var(--font-mono)">{labels[i]}</text>
        ))}
        {/* ΔT band */}
        <path d={bandPath} fill="var(--accent)" opacity="0.14"/>
        <path d={flowPath} fill="none" stroke="var(--hot)" strokeWidth="2"/>
        <path d={retPath}  fill="none" stroke="var(--cold)" strokeWidth="2"/>
        {hov != null && (
          <g>
            <line x1={x(hov)} y1={pad} x2={x(hov)} y2={h - pad} stroke="var(--text)" strokeDasharray="3 3" opacity="0.5"/>
            {data.flow[hov] != null && (
              <circle cx={x(hov)} cy={y(data.flow[hov])} r="3.5" fill="var(--hot)"/>
            )}
            {data.ret[hov] != null && (
              <circle cx={x(hov)} cy={y(data.ret[hov])} r="3.5" fill="var(--cold)"/>
            )}
          </g>
        )}
      </svg>
      <div className="ht-foot">
        <div className="ht-legend">
          <span><span className="sw" style={{background:"var(--hot)"}}/>Flow</span>
          <span><span className="sw" style={{background:"var(--cold)"}}/>Return</span>
          <span><span className="sw" style={{background:"var(--accent)",opacity:0.4}}/>ΔT band</span>
        </div>
        <div className="ht-tip">
          {hov != null
            ? (() => {
                const f = data.flow[hov], r = data.ret[hov];
                const delta = (f != null && r != null) ? f - r : null;
                return `${labels[hov]} — Flow ${formatDeg(f)} · Return ${formatDeg(r)} · ΔT ${formatDeg(delta)}`;
              })()
            : `Avg ΔT ${formatDeg(avgDelta)} over 24 h`}
        </div>
      </div>
    </div>
  );
};

const StepResponse = ({ data }) => {
  const [active, setActive] = React.useState(0);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const w = 1100, h = 280, pad = 38;
  const tMax = 1800;
  const allTemps = data.flatMap(e => e.samples.flatMap(s => [s.flow, s.ret]));
  const yMin = Math.floor(Math.min(...allTemps) - 1);
  const yMax = Math.ceil(Math.max(...allTemps) + 1);
  const x = t => pad + (t / tMax) * (w - pad * 1.5);
  const y = v => h - pad - ((v - yMin) / (yMax - yMin)) * (h - pad * 1.5);
  const avgT90 = data.reduce((a, e) => a + e.t90, 0) / data.length;
  const avgLag = data.reduce((a, e) => a + e.lag, 0) / data.length;

  return (
    <div className="step-response">
      <svg viewBox={`0 0 ${w} ${h}`} className="sr-svg">
        {/* gridlines */}
        {Array.from({length: 5}).map((_, j) => {
          const v = yMin + ((yMax - yMin) / 4) * j;
          return (
            <g key={j}>
              <line x1={pad} y1={y(v)} x2={w} y2={y(v)} stroke="var(--line)" strokeDasharray="2 4" opacity="0.5"/>
              <text x={pad - 6} y={y(v) + 3} textAnchor="end" fontSize="10" fill="var(--text-3)" fontFamily="var(--font-mono)">{v.toFixed(0)}°</text>
            </g>
          );
        })}
        {[0, 300, 600, 900, 1200, 1500, 1800].map(t => (
          <g key={t}>
            <line x1={x(t)} y1={pad} x2={x(t)} y2={h - pad} stroke="var(--line)" strokeDasharray="2 4" opacity="0.3"/>
            <text x={x(t)} y={h - pad + 14} textAnchor="middle" fontSize="10" fill="var(--text-3)" fontFamily="var(--font-mono)">
              {t === 0 ? "0" : `${(t/60).toFixed(0)}m`}
            </text>
          </g>
        ))}
        {/* faint historical events */}
        {data.map((e, i) => {
          if (i === active) return null;
          const fp = e.samples.map((s, k) => `${k ? "L" : "M"} ${x(s.t)} ${y(s.ret)}`).join(" ");
          return <path key={`g${i}`} d={fp} fill="none" stroke="var(--cold)" strokeWidth="1" opacity="0.18"/>;
        })}
        {/* active event */}
        {(() => {
          const e = data[active];
          const flowPath = e.samples.map((s, k) => `${k ? "L" : "M"} ${x(s.t)} ${y(s.flow)}`).join(" ");
          const retPath  = e.samples.map((s, k) => `${k ? "L" : "M"} ${x(s.t)} ${y(s.ret)}`).join(" ");
          return (
            <g>
              <path d={flowPath} fill="none" stroke="var(--hot)" strokeWidth="2"/>
              <path d={retPath}  fill="none" stroke="var(--cold)" strokeWidth="2"/>
              <line x1={x(e.t90)} y1={pad} x2={x(e.t90)} y2={h - pad}
                    stroke="var(--accent)" strokeWidth="1.2" strokeDasharray="4 3"/>
              <text x={x(e.t90) + 6} y={pad + 12} fontSize="10" fill="var(--accent)" fontFamily="var(--font-mono)">
                t₉₀ {(e.t90/60).toFixed(1)} min
              </text>
            </g>
          );
        })()}
      </svg>
      <div className="sr-foot">
        <div className="sr-pickers">
          {data.map((e, i) => (
            <button key={i}
                    className={`sr-event ${i === active ? "active" : ""}`}
                    onClick={() => setActive(i)}>
              <span className="t">Event {i + 1}</span>
              <span className="d">+{(e.sp1 - e.sp0).toFixed(0)}° · t₉₀ {(e.t90/60).toFixed(1)}m</span>
            </button>
          ))}
        </div>
        <div className="sr-stats">
          <div><span className="l">Avg lag</span> <span className="v">{avgLag.toFixed(0)}s</span></div>
          <div><span className="l">Avg t₉₀</span> <span className="v">{(avgT90/60).toFixed(1)}m</span></div>
          <div><span className="l">Events</span> <span className="v">{data.length}</span></div>
        </div>
      </div>
    </div>
  );
};
const Sparkbar = ({ data, color = "var(--accent)" }) => {
  // Render nothing while data is still loading.
  if (!Array.isArray(data) || data.length === 0) return <span className="sparkbar"/>;
  const max = Math.max(...data, 1);
  return (
    <span className="sparkbar">
      {data.map((v, i) => (
        <span key={i} className="sb"
              style={{ height: `${((v || 0) / max) * 100}%`, background: color }}/>
      ))}
    </span>
  );
};

/* ---------- Compressor starts per day (60-day bar chart) ---------- */
const StartsDaily = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0) return <EmptyChart/>;
  const max = Math.max(...data.map(d => d.starts), 1);
  return (
    <div className="starts-daily">
      <div className="sd-bars">
        {data.map((d, i) => {
          const high = d.starts > 14;
          return (
            <div key={i}
                 className={`sd-bar ${high ? "warn" : ""} ${hover === i ? "hover" : ""}`}
                 onMouseEnter={() => setHover(i)}
                 onMouseLeave={() => setHover(null)}>
              <div className="sd-fill" style={{ height: `${(d.starts / max) * 100}%` }}/>
            </div>
          );
        })}
      </div>
      <div className="sd-foot">
        <span className="sd-tip">
          {hover != null
            ? `${data[hover].date.toLocaleDateString("en-GB", { day:"numeric", month:"short" })} — ${data[hover].starts} starts · outdoor ${data[hover].outdoor.toFixed(1)}°C`
            : `60-day average: ${(data.reduce((a,b)=>a+b.starts,0)/data.length).toFixed(1)} starts/day`}
        </span>
        <span className="sd-legend">
          <span className="sw"/>typical &nbsp;
          <span className="sw warn"/>&gt; 14 starts/day
        </span>
      </div>
    </div>
  );
};

const StartsByHour = ({ data }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(data) || data.length === 0 || data.every(v => v === 0)) return <EmptyChart/>;
  const max = Math.max(...data, 1);
  return (
    <div className="starts-hour">
      <div className="sh-bars">
        {data.map((v, h) => (
          <div key={h}
               className={`sh-bar ${hover === h ? "hover" : ""}`}
               onMouseEnter={() => setHover(h)}
               onMouseLeave={() => setHover(null)}>
            <div className="sh-fill" style={{ height: `${(v / max) * 100}%` }}/>
            <div className="sh-lbl">{h % 6 === 0 ? h.toString().padStart(2,"0") : ""}</div>
          </div>
        ))}
      </div>
      <div className="sh-foot">
        {hover != null
          ? `${hover.toString().padStart(2,"0")}:00 — ${data[hover]} starts (sum across last 60 days)`
          : "Two daily peaks: morning (DHW) and evening (heating)"}
      </div>
    </div>
  );
};

const StartsVsTemp = ({ data: rawData }) => {
  const [hover, setHover] = React.useState(null);
  if (!Array.isArray(rawData)) return <EmptyChart/>;
  // Drop null-outdoor rows so they don't cluster at x=0 on the scatter.
  const data = rawData.filter(d => d.outdoor != null);
  if (data.length < 2) return <EmptyChart/>;
  const w = 360, h = 220, pad = 28;
  const tMin = Math.min(...data.map(d => d.outdoor)) - 1;
  const tMax = Math.max(...data.map(d => d.outdoor)) + 1;
  const sMax = Math.max(...data.map(d => d.starts)) + 1;
  const x = t => pad + ((t - tMin) / (tMax - tMin)) * (w - pad * 1.5);
  const y = s => h - pad - (s / sMax) * (h - pad * 1.5);
  // simple least-squares regression for the trend line
  const n = data.length;
  const mx = data.reduce((a, d) => a + d.outdoor, 0) / n;
  const my = data.reduce((a, d) => a + d.starts,  0) / n;
  let num = 0, den = 0;
  data.forEach(d => { num += (d.outdoor - mx) * (d.starts - my); den += (d.outdoor - mx) ** 2; });
  // den === 0 when every outdoor sample is identical — no variance, no
  // regression. Skip the trend line in that case instead of plotting NaN.
  const hasTrend = den > 0;
  const slope = hasTrend ? num / den : 0;
  const intercept = hasTrend ? my - slope * mx : 0;
  const lineY = t => slope * t + intercept;
  return (
    <div className="starts-scatter">
      <svg viewBox={`0 0 ${w} ${h}`} className="ss-svg">
        {/* axes */}
        <line x1={pad} y1={h - pad} x2={w} y2={h - pad} stroke="var(--line)" strokeWidth="1"/>
        <line x1={pad} y1={pad} x2={pad} y2={h - pad} stroke="var(--line)" strokeWidth="1"/>
        {/* gridlines */}
        {[0, 5, 10, 15, 20].map(s => s <= sMax && (
          <g key={s}>
            <line x1={pad} y1={y(s)} x2={w} y2={y(s)} stroke="var(--line)" strokeDasharray="2 4" opacity="0.4"/>
            <text x={pad - 4} y={y(s) + 3} textAnchor="end" fontSize="9" fill="var(--text-3)" fontFamily="var(--font-mono)">{s}</text>
          </g>
        ))}
        {[0, 5, 10, 15, 20].map(t => t >= tMin && t <= tMax && (
          <text key={t} x={x(t)} y={h - pad + 12} textAnchor="middle" fontSize="9" fill="var(--text-3)" fontFamily="var(--font-mono)">{t}°</text>
        ))}
        {/* trend line — omitted when outdoor variance is zero */}
        {hasTrend && (
          <line x1={x(tMin)} y1={y(lineY(tMin))} x2={x(tMax)} y2={y(lineY(tMax))}
                stroke="var(--accent)" strokeWidth="1.5" strokeDasharray="4 3" opacity="0.7"/>
        )}
        {/* points */}
        {data.map((d, i) => {
          const expected = lineY(d.outdoor);
          const dev = Math.abs(d.starts - expected) / Math.max(expected, 1);
          // No regression line ⇒ no "expected" to compare against, so don't
          // flag outliers.
          const outlier = hasTrend && dev > 0.5;
          return (
            <circle key={i} cx={x(d.outdoor)} cy={y(d.starts)}
                    r={hover === i ? 4.5 : 3}
                    fill={outlier ? "oklch(0.7 0.18 30)" : "var(--cold)"}
                    stroke={hover === i ? "var(--text)" : "transparent"}
                    strokeWidth="1.5"
                    onMouseEnter={() => setHover(i)}
                    onMouseLeave={() => setHover(null)}/>
          );
        })}
      </svg>
      <div className="ss-foot">
        {hover != null
          ? `${data[hover].outdoor.toFixed(1)}°C — ${data[hover].starts} starts on ${data[hover].date.toLocaleDateString("en-GB",{day:"numeric",month:"short"})}`
          : hasTrend
          ? `Trend: ${slope.toFixed(2)} starts per °C colder · R² is left as an exercise`
          : "Not enough outdoor variance for a trend line"}
      </div>
    </div>
  );
};

/* expose to other Babel scripts */
Object.assign(window, {
  CycleHistogram,
  HourHeatmap,
  CalendarHeatmap,
  MonthlyStacked,
  RadialClock,
  StartsDaily,
  StartsByHour,
  StartsVsTemp,
  HeatingTrend,
  StepResponse,
  Sparkbar,
});
