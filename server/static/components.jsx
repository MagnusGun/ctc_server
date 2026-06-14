/* Small reusable components and inline icons */

const Icon = ({ name, size = 16 }) => {
  const paths = {
    bolt:    <path d="M11 2 4 13h6l-1 7 7-11h-6l1-7Z" />,
    flame:   <path d="M12 2c0 4-5 5-5 10a5 5 0 0 0 10 0c0-3-2-3-2-6 0 2-3 2-3-4Z" />,
    thermo:  <><circle cx="12" cy="17" r="3"/><path d="M12 4v10"/></>,
    cog:     <><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/></>,
    pump:    <><circle cx="12" cy="12" r="8"/><path d="M12 4v8l5 3"/></>,
    chart:   <path d="M3 17l5-5 4 4 8-9" />,
    msg:     <path d="M4 5h16v11H8l-4 4V5Z" />,
    alert:   <><path d="M12 4 2 20h20L12 4Z"/><path d="M12 10v5M12 18h.01"/></>,
    download:<><path d="M12 3v12"/><path d="M7 11l5 4 5-4"/><path d="M4 19h16"/></>,
    zap:     <path d="M11 2 4 13h6l-1 7 7-11h-6l1-7Z" />,
  };
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none"
         stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      {paths[name]}
    </svg>
  );
};

const Card = ({
  icon, title, actions, banner, children, className = "",
  collapsible = false, collapsed = false, onToggleCollapse,
}) => (
  <section className={`card ${collapsed ? "collapsed" : ""} ${className}`}>
    <header className="card-head">
      <h2 className="card-title">
        {icon ? <span className="ico"><Icon name={icon} size={15}/></span> : null}
        {title}
      </h2>
      <div className="card-actions">
        {banner && !collapsed ? <span className="stats-banner">{banner}</span> : null}
        {!collapsed && actions}
        {collapsible && (
          <button type="button" className="card-toggle"
                  onClick={onToggleCollapse}
                  aria-expanded={!collapsed}
                  aria-label={`${collapsed ? "Expand" : "Collapse"} ${title}`}>
            <span className="chev" aria-hidden="true"/>
          </button>
        )}
      </div>
    </header>
    <div className="card-body">{children}</div>
  </section>
);

const Metric = ({ label, value, unit, sub, tone, featured, sparkData, hint, onClick }) => (
  <div className={`metric ${tone || ""} ${featured ? "featured" : ""} ${onClick ? "clickable" : ""}`}
       onClick={onClick} role={onClick ? "button" : undefined} tabIndex={onClick ? 0 : undefined}
       onKeyDown={onClick ? (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onClick(); } } : undefined}>
    <div className="label">
      {hint ? (
        <span className="has-tip" tabIndex="0" onClick={(e) => e.stopPropagation()}>
          {label}
          <span className="tip">{hint}</span>
        </span>
      ) : label}
      {onClick ? <span className="chart-affordance" aria-hidden="true">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
             stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M3 17l5-5 4 4 8-9"/>
        </svg>
      </span> : null}
    </div>
    <div className="value">
      {value}
      {unit ? <span className="unit">{unit}</span> : null}
    </div>
    {sub ? <div className="sub">{sub}</div> : null}
    {sparkData ? <Sparkline data={sparkData} tone={tone} /> : null}
  </div>
);

const Tip = ({ children, hint }) => (
  <span className="has-tip" tabIndex="0">
    {children}
    <span className="tip">{hint}</span>
  </span>
);

// Build polyline segments from a numeric series, breaking on null/NaN so gaps
// render as gaps instead of straight lines through zero.
const sparkSegments = (data, w, h, min, range) => {
  const segs = [];
  let cur = [];
  const len = data?.length ?? 0;
  for (let i = 0; i < len; i++) {
    const d = data[i];
    if (d == null || Number.isNaN(d)) {
      if (cur.length > 1) segs.push(cur);
      cur = [];
    } else {
      const x = (i / (len - 1)) * w;
      const y = h - ((d - min) / range) * (h - 4) - 2;
      cur.push(`${x},${y}`);
    }
  }
  if (cur.length > 1) segs.push(cur);
  return segs;
};

const SPARK_TONE_STROKE = {
  hot:  "var(--hot)",
  cool: "var(--cold)",
  warm: "oklch(0.85 0.13 60)",
  good: "var(--good)",
};

const Sparkline = ({ data, tone, color }) => {
  const w = 200, h = 22;
  const values = (data || []).filter(v => v != null && !Number.isNaN(v));
  if (values.length === 0) return null;
  const min = Math.min(...values), max = Math.max(...values);
  const range = max - min || 1;
  const segments = sparkSegments(data, w, h, min, range);
  const stroke = color ?? SPARK_TONE_STROKE[tone] ?? "var(--text-3)";
  return (
    <svg className="spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none">
      {segments.map((seg, i) => (
        <polyline key={i} fill="none" stroke={stroke} strokeWidth="1.4" points={seg.join(" ")} opacity="0.85"/>
      ))}
    </svg>
  );
};

/* Multi-series miniature sparkline used by the heating-system card thumbnail
   to show flow / return / outdoor for the last 24h. */
const MultiSparkline = ({ series }) => {
  const w = 70, h = 18;
  const values = series.flatMap(s => s.data || []).filter(v => v != null && !Number.isNaN(v));
  if (values.length === 0) return <svg className="spark" viewBox={`0 0 ${w} ${h}`}/>;
  const min = Math.min(...values), max = Math.max(...values);
  const range = max - min || 1;
  return (
    <svg className="spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none">
      {series.map((s, i) =>
        sparkSegments(s.data, w, h, min, range).map((seg, j) => (
          <polyline key={`${i}-${j}`} fill="none" stroke={s.color}
                    strokeWidth="1.2" points={seg.join(" ")} opacity="0.85"/>
        ))
      )}
    </svg>
  );
};

const Pill = ({ children, tone }) => (
  <span className={`hp-pill ${tone || ""}`}>{children}</span>
);

const PumpBar = ({ label, pct }) => (
  <div className="pump">
    <span className="lbl">{label}</span>
    <span className="bar"><i style={{ width: `${pct}%` }} /></span>
    <span className="pct">{pct}%</span>
  </div>
);

const Brand = ({ state = "connected" }) => {
  const label = state === "offline" ? "Offline"
              : state === "connecting" ? "Connecting"
              : "Connected";
  return (
    <Tip hint={`CTC EcoHeat 410 · ${label}`}>
      <div className="brand" data-state={state} role="status" aria-label={`CTC EcoHeat 410, ${label}`}>
        <div className="brand-mark">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
               stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M11 2 4 13h6l-1 7 7-11h-6l1-7Z" />
          </svg>
        </div>
        <div className="brand-text">
          <span className="product">CTC</span>
          <span className="model">EcoHeat 410</span>
        </div>
      </div>
    </Tip>
  );
};

Object.assign(window, { Icon, Card, Metric, Sparkline, MultiSparkline, Pill, PumpBar, Brand, Tip });
