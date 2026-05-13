/* Energy spot price chart — today's curve, now-line, current bar highlight */

const EnergyChart = ({ today, nowIndex, height = 200 }) => {
  const [hover, setHover] = React.useState(null);
  const w = 1200;
  const h = height;
  const padL = 40, padR = 20, padT = 16, padB = 26;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;

  // Render an empty-state placeholder when the price feed hasn't filled in.
  const hasData = Array.isArray(today) && today.some(v => v != null && v > 0);

  const yMin = 0;
  const yMax = hasData
    ? Math.ceil(Math.max(...today.filter(v => v != null)) * 10) / 10 + 0.1
    : 1;

  const xFor = i => padL + (i / 24) * innerW;
  const yFor = v => padT + innerH - ((v - yMin) / (yMax - yMin)) * innerH;

  const buildPath = (arr) => {
    let d = "";
    arr.forEach((v, i) => {
      if (v == null) return;
      d += (d ? "L" : "M") + xFor(i) + "," + yFor(v) + " ";
    });
    return d;
  };

  const buildArea = (arr) => {
    const top = buildPath(arr);
    if (!top) return "";
    // Find first/last non-null index for the area baseline.
    const firstIdx = arr.findIndex(v => v != null);
    let lastIdx = -1;
    for (let i = arr.length - 1; i >= 0; i--) {
      if (arr[i] != null) { lastIdx = i; break; }
    }
    return top
      + ` L ${xFor(lastIdx)},${yFor(yMin)}`
      + ` L ${xFor(firstIdx)},${yFor(yMin)} Z`;
  };

  // Y axis ticks
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

  const nowX = xFor(nowIndex);
  const nowIdxFloor = Math.floor(nowIndex);
  const nowVal = hasData ? today[nowIdxFloor] : null;

  return (
    <svg className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
      <defs>
        <linearGradient id="todayFill" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0%"   stopColor="var(--accent)" stopOpacity="0.28"/>
          <stop offset="100%" stopColor="var(--accent)" stopOpacity="0"/>
        </linearGradient>
      </defs>

      {/* gridlines */}
      {ticks.map((t, i) => (
        <g key={`gy${i}`}>
          <line className="grid-line" x1={padL} x2={w - padR} y1={t.y} y2={t.y} />
          <text className="axis-label" x={padL - 8} y={t.y + 3} textAnchor="end">
            {t.v.toFixed(2)}
          </text>
        </g>
      ))}

      {/* x labels */}
      {xTicks.map((t, i) => (
        <text key={`xt${i}`} className="axis-label" x={t.x} y={h - 8} textAnchor="middle">
          {t.label}
        </text>
      ))}

      {/* today */}
      {hasData && (
        <>
          <path d={buildArea(today)} fill="url(#todayFill)" />
          <path d={buildPath(today)} fill="none" stroke="var(--accent)" strokeWidth="1.8" />
          {today.map((v, i) => (
            v != null && (
              <circle key={`d${i}`} cx={xFor(i)} cy={yFor(v)} r={i === nowIdxFloor ? 4 : 2}
                      fill={i === nowIdxFloor ? "var(--accent)" : "var(--bg)"}
                      stroke="var(--accent)" strokeWidth="1.4"/>
            )
          ))}
        </>
      )}

      {/* now line — only when we have a real value to anchor to */}
      {hasData && nowVal != null && (
        <>
          <line className="now-line" x1={nowX} x2={nowX} y1={padT} y2={h - padB} />
          <text className="now-label" x={nowX} y={padT - 4} textAnchor="middle">NOW</text>
        </>
      )}

      {!hasData && (
        <text x={w / 2} y={h / 2} textAnchor="middle" dominantBaseline="middle"
              fill="var(--text-3)" fontSize="12" fontStyle="italic">
          No price data yet
        </text>
      )}

      {/* hover capture */}
      <rect x={padL} y={padT} width={innerW} height={innerH}
        fill="transparent"
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const ratio = (e.clientX - rect.left) / rect.width;
          const i = Math.max(0, Math.min(23, Math.round(ratio * 23)));
          setHover(i);
        }}/>

      {hasData && hover != null && today[hover] != null && (
        <g pointerEvents="none">
          <line x1={xFor(hover)} x2={xFor(hover)} y1={padT} y2={h-padB}
                stroke="var(--text-3)" strokeDasharray="2 3" strokeWidth="1"/>
          <circle cx={xFor(hover)} cy={yFor(today[hover])} r="5"
                  fill="var(--accent)" stroke="var(--bg)" strokeWidth="2"/>
          {(() => {
            const tx = xFor(hover);
            const ty = yFor(today[hover]) - 14;
            const lbl = `${String(hover).padStart(2,"0")}:00 — ${today[hover].toFixed(2)} kr/kWh`;
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
