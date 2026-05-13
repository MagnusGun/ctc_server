/* Energy spot price chart — today's curve, now-line, current bar highlight. */

const LEVEL_VAR = {
  very_cheap:     "var(--price-very-cheap)",
  cheap:          "var(--price-cheap)",
  normal:         "var(--price-normal)",
  expensive:      "var(--price-expensive)",
  very_expensive: "var(--price-very-expensive)",
};
const levelColor = lvl => LEVEL_VAR[lvl] ?? "var(--text-3)";

const EnergyChart = ({ today, nowIndex, height = 200 }) => {
  const [hover, setHover] = React.useState(null);
  const w = 1200;
  const h = height;
  const padL = 40, padR = 20, padT = 16, padB = 26;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;

  const slots = Array.isArray(today) ? today : [];
  const N = slots.length;
  const hasData = slots.some(p => p?.spot_sek != null && p.spot_sek > 0);

  const yMin = 0;
  const yMax = hasData
    ? Math.ceil(Math.max(...slots.map(p => p?.spot_sek).filter(v => v != null)) * 10) / 10 + 0.1
    : 1;

  const xFor = i => padL + (N > 0 ? (i / N) * innerW : 0);
  const yFor = v => padT + innerH - ((v - yMin) / (yMax - yMin)) * innerH;

  const buildArea = () => {
    let d = "";
    let firstIdx = -1, lastIdx = -1;
    slots.forEach((p, i) => {
      const v = p?.spot_sek;
      if (v == null) return;
      if (firstIdx < 0) firstIdx = i;
      lastIdx = i;
      d += (d ? "L" : "M") + xFor(i) + "," + yFor(v) + " ";
    });
    if (!d) return "";
    return d
      + ` L ${xFor(lastIdx)},${yFor(yMin)}`
      + ` L ${xFor(firstIdx)},${yFor(yMin)} Z`;
  };

  const ticks = [];
  const tickCount = 4;
  for (let i = 0; i <= tickCount; i++) {
    const v = yMin + (yMax - yMin) * (i / tickCount);
    ticks.push({ v, y: yFor(v) });
  }

  // X labels every 2h. Final tick reads "00:00" (next day), not "24:00".
  const xTicks = [];
  for (let i = 0; i <= 24; i += 2) {
    xTicks.push({
      label: `${String(i % 24).padStart(2, "0")}:00`,
      x: padL + (i / 24) * innerW,
    });
  }

  const nowIdxFloor = Math.floor(nowIndex);
  const nowSlot = hasData && nowIdxFloor < N ? slots[nowIdxFloor] : null;
  const nowVal = nowSlot?.spot_sek ?? null;
  const nowX = xFor(nowIndex);

  return (
    <svg className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
      <defs>
        <linearGradient id="todayFill" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0%"   stopColor="var(--text-3)" stopOpacity="0.18"/>
          <stop offset="100%" stopColor="var(--text-3)" stopOpacity="0"/>
        </linearGradient>
      </defs>

      {ticks.map((t, i) => (
        <g key={`gy${i}`}>
          <line className="grid-line" x1={padL} x2={w - padR} y1={t.y} y2={t.y} />
          <text className="axis-label" x={padL - 8} y={t.y + 3} textAnchor="end">
            {t.v.toFixed(2)}
          </text>
        </g>
      ))}

      {xTicks.map((t, i) => (
        <text key={`xt${i}`} className="axis-label" x={t.x} y={h - 8} textAnchor="middle">
          {t.label}
        </text>
      ))}

      {hasData && (
        <>
          <path d={buildArea()} fill="url(#todayFill)" />
          {slots.slice(0, -1).map((p, i) => {
            const a = p?.spot_sek;
            const b = slots[i + 1]?.spot_sek;
            if (a == null || b == null) return null;
            return (
              <line key={`s${i}`}
                    x1={xFor(i)}     y1={yFor(a)}
                    x2={xFor(i + 1)} y2={yFor(b)}
                    stroke={levelColor(p.level)}
                    strokeWidth="2"
                    strokeLinecap="round"/>
            );
          })}
        </>
      )}

      {/* now line — only when we have a real value to anchor to */}
      {hasData && nowVal != null && (
        <>
          <line className="now-line" x1={nowX} x2={nowX} y1={padT} y2={h - padB} />
          <text className="now-label" x={nowX} y={padT - 4} textAnchor="middle">NOW</text>
          <circle cx={xFor(nowIdxFloor)} cy={yFor(nowVal)} r="4"
                  fill={levelColor(nowSlot.level)}
                  stroke="var(--bg)" strokeWidth="1.5"/>
        </>
      )}

      {!hasData && (
        <text x={w / 2} y={h / 2} textAnchor="middle" dominantBaseline="middle"
              fill="var(--text-3)" fontSize="12" fontStyle="italic">
          No price data yet
        </text>
      )}

      <rect x={padL} y={padT} width={innerW} height={innerH}
        fill="transparent"
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          if (N === 0) return;
          const rect = e.currentTarget.getBoundingClientRect();
          const ratio = (e.clientX - rect.left) / rect.width;
          const i = Math.max(0, Math.min(N - 1, Math.round(ratio * (N - 1))));
          if (i !== hover) setHover(i);
        }}/>

      {hasData && hover != null && slots[hover]?.spot_sek != null && (
        <g pointerEvents="none">
          <line x1={xFor(hover)} x2={xFor(hover)} y1={padT} y2={h-padB}
                stroke="var(--text-3)" strokeDasharray="2 3" strokeWidth="1"/>
          <circle cx={xFor(hover)} cy={yFor(slots[hover].spot_sek)} r="5"
                  fill={levelColor(slots[hover].level)}
                  stroke="var(--bg)" strokeWidth="2"/>
          {(() => {
            const p = slots[hover];
            const tx = xFor(hover);
            const ty = yFor(p.spot_sek) - 14;
            const timeRange = `${window.formatHM(p.starts_at)}–${window.formatHM(p.ends_at)}`;
            const levelTxt = window.PRICE_LEVEL_LABELS?.[p.level] ?? "";
            const lbl = `${timeRange} · ${p.spot_sek.toFixed(2)} kr/kWh${levelTxt ? " · " + levelTxt : ""}`;
            const tw = lbl.length * 6.2 + 16;
            const left = tx - tw/2 < padL ? padL : (tx + tw/2 > w - padR ? w - padR - tw : tx - tw/2);
            return (
              <g transform={`translate(${left}, ${Math.max(padT+4, ty - 22)})`}>
                <rect width={tw} height="22" rx="4"
                      fill="oklch(0.10 0.005 250)" stroke="var(--line-2)"/>
                <text x={tw/2} y="14" textAnchor="middle"
                      fill="oklch(0.96 0.005 250)" fontSize="11"
                      fontFamily="var(--font-mono)" fontWeight="500">
                  {lbl}
                </text>
              </g>
            );
          })()}
        </g>
      )}
    </svg>
  );
};

window.EnergyChart = EnergyChart;
