/* Activity timeline + multi-series temperature trend */

const ActivityTimeline = ({ segments, height = 60, hours = 24 }) => {
  const w = 1200;
  const h = height;
  const padL = 40, padR = 20, padT = 8, padB = 22;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;
  const lanes = ["Heating", "DHW", "Brine"];
  const laneH = innerH / lanes.length;

  const xFor = (hour) => padL + (hour / hours) * innerW;
  const colorFor = (lane) =>
    lane === "Heating" ? "var(--accent)"
    : lane === "DHW"   ? "var(--hot)"
    : "var(--cold)";

  const xTicks = [];
  for (let i = 0; i <= hours; i += 2) {
    xTicks.push({ label: `${String(i).padStart(2,"0")}:00`, x: xFor(i) });
  }

  return (
    <svg className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
      {/* lane labels + bg */}
      {lanes.map((lane, i) => {
        const y = padT + i * laneH;
        return (
          <g key={lane}>
            <rect x={padL} y={y + 4} width={innerW} height={Math.max(2, laneH - 8)}
                  fill="var(--surface-2)" rx="3" opacity="0.6"/>
            <text x={padL - 8} y={y + laneH / 2 + 3.5} textAnchor="end"
                  className="axis-label">
              {lane}
            </text>
          </g>
        );
      })}

      {/* x ticks */}
      {xTicks.map((t, i) => (
        <g key={`xt${i}`}>
          <line x1={t.x} x2={t.x} y1={padT} y2={padT + innerH}
                stroke="var(--line)" strokeDasharray="2 4" opacity="0.5"/>
          <text x={t.x} y={h - 6} textAnchor="middle" className="axis-label">
            {t.label}
          </text>
        </g>
      ))}

      {/* segments */}
      {segments.map((s, i) => {
        const laneIdx = lanes.indexOf(s.lane);
        const y = padT + laneIdx * laneH + 6;
        const x = xFor(s.start);
        const segW = xFor(s.end) - x;
        return (
          <g key={i}>
            <rect x={x} y={y} width={Math.max(2, segW)} height={Math.max(2, laneH - 12)}
                  rx="3" fill={colorFor(s.lane)} opacity="0.85">
              <title>{`${s.lane} · ${formatHr(s.start)}–${formatHr(s.end)} · ${((s.end-s.start)*60).toFixed(0)} min`}</title>
            </rect>
          </g>
        );
      })}
    </svg>
  );
};

const formatHr = (h) => {
  const hh = Math.floor(h);
  const mm = Math.round((h - hh) * 60);
  return `${String(hh).padStart(2,"0")}:${String(mm).padStart(2,"0")}`;
};

const TrendChart = ({ series, height = 200, yMin, yMax, hours = 24, unit = "°C" }) => {
  const [hover, setHover] = React.useState(null);
  const w = 1200;
  const h = height;
  const padL = 44, padR = 20, padT = 16, padB = 26;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;

  // Defensive: empty series shouldn't render. The fetch-failed path can leave
  // the modal open without data.
  if (!Array.isArray(series) || series.length === 0 || !series[0]?.data?.length) {
    return (
      <svg className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
        <text x={w / 2} y={h / 2} textAnchor="middle" dominantBaseline="middle"
              fill="var(--text-3)" fontSize="12" fontStyle="italic">
          No data yet
        </text>
      </svg>
    );
  }

  // Null buckets are gaps in the series (sensor offline, cache miss).
  // Filter them out of axis computation; Math.min/max(...[null, null]) yields
  // ±Infinity which then poisons every yFor() call.
  const all = series.flatMap(s => s.data).filter(v => v != null);
  // yMin/yMax act as a soft minimum span: the chart shows at least that many
  // units, centered on the data when the range is narrower. When data extends
  // past the soft window (cold snap below 8 °C, brine ΔT spike), the axis
  // expands to fit. Works with negatives.
  let min, max;
  if (all.length === 0) {
    min = yMin ?? 0;
    max = yMax ?? 1;
  } else {
    const dataMin = Math.min(...all);
    const dataMax = Math.max(...all);
    const pad = Math.max(1, (dataMax - dataMin) * 0.1);
    let visMin = dataMin - pad;
    let visMax = dataMax + pad;
    const softSpan = (yMin != null && yMax != null) ? (yMax - yMin) : 0;
    if (softSpan > 0 && (visMax - visMin) < softSpan) {
      const center = (dataMin + dataMax) / 2;
      visMin = center - softSpan / 2;
      visMax = center + softSpan / 2;
    }
    min = Math.floor(visMin);
    max = Math.ceil(visMax);
  }

  const xFor = (i, len) => padL + (i / (len - 1)) * innerW;
  const yFor = v => padT + innerH - ((v - min) / (max - min)) * innerH;

  const ticks = [];
  for (let i = 0; i <= 4; i++) {
    const v = min + (max - min) * (i / 4);
    ticks.push({ v, y: yFor(v) });
  }
  const xTicks = [];
  for (let i = 0; i <= hours; i += 4) {
    xTicks.push({ label: `${String(i).padStart(2,"0")}:00`, x: padL + (i / hours) * innerW });
  }

  const refLen = series[0].data.length;

  return (
    <svg className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
      {/* gridlines */}
      {ticks.map((t, i) => (
        <g key={`gy${i}`}>
          <line className="grid-line" x1={padL} x2={w - padR} y1={t.y} y2={t.y}/>
          <text className="axis-label" x={padL - 8} y={t.y + 3} textAnchor="end">
            {t.v.toFixed(0)}
          </text>
        </g>
      ))}
      {xTicks.map((t, i) => (
        <text key={`xt${i}`} className="axis-label" x={t.x} y={h - 8} textAnchor="middle">
          {t.label}
        </text>
      ))}

      {/* lines — null buckets break the path so gaps render as gaps,
          not as zero-crossing lines through the floor of the chart. */}
      {series.map((s, si) => {
        let started = false;
        const path = s.data
          .map((v, i) => {
            if (v == null) {
              started = false;
              return "";
            }
            const cmd = started ? "L" : "M";
            started = true;
            return `${cmd}${xFor(i, s.data.length)},${yFor(v)}`;
          })
          .join(" ");
        return (
          <path key={si} d={path} fill="none" stroke={s.color}
                strokeWidth="1.8" strokeLinejoin="round"/>
        );
      })}

      {/* hover capture */}
      <rect x={padL} y={padT} width={innerW} height={innerH}
        fill="transparent"
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const ratio = (e.clientX - rect.left) / rect.width;
          const i = Math.max(0, Math.min(refLen - 1, Math.round(ratio * (refLen - 1))));
          setHover(i);
        }}/>

      {hover != null && (
        <g pointerEvents="none">
          <line x1={xFor(hover, refLen)} x2={xFor(hover, refLen)} y1={padT} y2={h-padB}
                stroke="var(--text-3)" strokeDasharray="2 3" strokeWidth="1"/>
          {series.map((s, si) =>
            s.data[hover] == null ? null : (
              <circle key={si} cx={xFor(hover, s.data.length)} cy={yFor(s.data[hover])}
                      r="4" fill={s.color} stroke="var(--bg)" strokeWidth="2"/>
            )
          )}
          {(() => {
            const tx = xFor(hover, refLen);
            // Format null buckets as "—" rather than crashing `.toFixed`.
            const formatValue = (v) => (v == null ? "—" : `${v.toFixed(1)}${unit}`);
            const lines = series.map(s => `${s.label}: ${formatValue(s.data[hover])}`);
            const tw = Math.max(...lines.map(l => l.length)) * 6.4 + 26;
            const th = lines.length * 14 + 18;
            const left = tx + 12 + tw > w - padR ? tx - 12 - tw : tx + 12;
            return (
              <g transform={`translate(${left}, ${padT + 6})`}>
                <rect width={tw} height={th} rx="5"
                      fill="oklch(0.10 0.005 250)" stroke="var(--line-2)"/>
                <text x="10" y="15" fill="oklch(0.78 0.008 250)" fontSize="10"
                      fontFamily="var(--font-mono)">
                  {`${String(hover).padStart(2,"0")}:00`}
                </text>
                {series.map((s, si) => (
                  <g key={si} transform={`translate(10, ${28 + si * 14})`}>
                    <rect x="0" y="-7" width="6" height="6" rx="1" fill={s.color}/>
                    <text x="12" y="-1" fill="oklch(0.96 0.005 250)" fontSize="11"
                          fontFamily="var(--font-mono)">
                      {`${s.label}: ${formatValue(s.data[hover])}`}
                    </text>
                  </g>
                ))}
              </g>
            );
          })()}
        </g>
      )}
    </svg>
  );
};

Object.assign(window, { ActivityTimeline, TrendChart });
