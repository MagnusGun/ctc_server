/* Energy spot price chart — today's curve, now-line, current bar highlight. */

const LEVEL_VAR = {
  very_cheap:     "var(--price-very-cheap)",
  cheap:          "var(--price-cheap)",
  normal:         "var(--price-normal)",
  expensive:      "var(--price-expensive)",
  very_expensive: "var(--price-very-expensive)",
};
const levelColor = lvl => LEVEL_VAR[lvl] ?? "var(--text-3)";

const EnergyChart = React.memo(({ today, nowIndex, scheduledResumeAt = null, scheduledRunMinutes = null, dhwBoost = null, height = 200 }) => {
  const [hover, setHover] = React.useState(null);
  const [containerRef, measuredW] = window.useElementSize();
  const w = measuredW || 1200;
  const h = height;
  const padL = 40, padR = 20, padT = 16, padB = 26;
  const innerW = w - padL - padR;
  const innerH = h - padT - padB;

  const slots = Array.isArray(today) ? today : [];
  const N = slots.length;
  const hasData = slots.some(p => p?.spot_sek != null && p.spot_sek > 0);

  // Scheduled-run overlay. The visibility check uses the actual slot time
  // domain (not wall-clock "today") so the band is robust to brief
  // mismatches around midnight when server-side price rollover lags the
  // calendar by a few seconds.
  const resumeBand = (() => {
    if (!hasData || !scheduledResumeAt || !(scheduledRunMinutes > 0) || N < 1) return null;
    const startMs = Date.parse(slots[0]?.starts_at);
    const endMs = Date.parse(slots[N - 1]?.ends_at);
    const resumeMs = Date.parse(scheduledResumeAt);
    if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || !Number.isFinite(resumeMs)) return null;
    if (!(endMs > startMs) || resumeMs < startMs || resumeMs >= endMs) return null;
    const span = endMs - startMs;
    const xLeft = padL + ((resumeMs - startMs) / span) * innerW;
    const widthRaw = (scheduledRunMinutes * 60_000 / span) * innerW;
    const xRight = Math.min(padL + innerW, xLeft + widthRaw);
    const bandW = Math.max(2, xRight - xLeft);
    const resumeHM = window.formatHM(scheduledResumeAt);
    const endIso = new Date(resumeMs + scheduledRunMinutes * 60_000).toISOString();
    const endHM = window.formatHM(endIso);
    const labelX = xLeft + 4;
    const showLabel = bandW > 40;
    return (
      <g className="resume-band-group">
        <title>{`Heat pump resumes at ${resumeHM} · runs ${scheduledRunMinutes} min (until ~${endHM})\nCheapest contiguous window picked by auto-resume scheduler.`}</title>
        <rect className="resume-band" x={xLeft} y={padT} width={bandW} height={innerH} />
        <rect className="resume-band-stripe" x={xLeft} y={padT} width={bandW} height={innerH} />
        <line className="resume-band-edge" x1={xLeft} x2={xLeft} y1={padT} y2={h - padB} />
        {showLabel && (
          <>
            <rect className="resume-band-label-bg" x={labelX - 3} y={padT + 2} width={Math.min(bandW - 4, 64)} height={14} rx="2"/>
            <text className="resume-band-label" x={labelX} y={padT + 12}>▶ {resumeHM}</text>
          </>
        )}
      </g>
    );
  })();

  // DHW boost overlay. Renders whenever an active boost's [started_at,
  // scheduled_end] interval has any overlap with the chart's slot range,
  // clipped to the chart bounds. Visual is OKLCH warm-orange fill at
  // alpha 0.18; CSS comment in styles.css notes the SmartGrid overlap.
  const dhwBand = (() => {
    if (!hasData || !dhwBoost || N < 1) return null;
    const slotStart = Date.parse(slots[0]?.starts_at);
    const slotEnd = Date.parse(slots[N - 1]?.ends_at);
    const boostStart = Date.parse(dhwBoost.started_at);
    const boostEnd = Date.parse(dhwBoost.scheduled_end);
    if (!Number.isFinite(slotStart) || !Number.isFinite(slotEnd)
        || !Number.isFinite(boostStart) || !Number.isFinite(boostEnd)) return null;
    // Clip the boost interval to the chart's domain. If they don't overlap,
    // skip rendering — the badge already communicates the boost.
    const left = Math.max(boostStart, slotStart);
    const right = Math.min(boostEnd, slotEnd);
    if (!(right > left) || !(slotEnd > slotStart)) return null;
    const span = slotEnd - slotStart;
    const xLeft = padL + ((left - slotStart) / span) * innerW;
    const xRight = padL + ((right - slotStart) / span) * innerW;
    const bandW = Math.max(2, xRight - xLeft);
    return (
      <g pointerEvents="none">
        <rect className="dhw-boost-band" x={xLeft} y={padT}
              width={bandW} height={innerH} />
      </g>
    );
  })();

  const yMin = 0;
  const yMax = hasData
    ? Math.ceil(Math.max(...slots.map(p => p?.spot_sek).filter(v => v != null)) * 10) / 10 + 0.1
    : 1;

  const xFor = i => padL + (N > 0 ? (i / N) * innerW : 0);
  const yFor = v => padT + innerH - ((v - yMin) / (yMax - yMin)) * innerH;

  const ticks = [];
  const tickCount = 4;
  for (let i = 0; i <= tickCount; i++) {
    const v = yMin + (yMax - yMin) * (i / tickCount);
    ticks.push({ v, y: yFor(v) });
  }

  // X labels every 2h. Final tick reads "00:00" (next day), not "24:00".
  // Compact form ("06" instead of "06:00") kicks in when the chart is narrow
  // enough that the wide form would collide; the tick count stays at 13.
  const compactLabels = w < window.COMPACT_CHART_WIDTH;
  const xTicks = [];
  for (let i = 0; i <= 24; i += 2) {
    const hh = String(i % 24).padStart(2, "0");
    xTicks.push({
      label: compactLabels ? hh : `${hh}:00`,
      x: padL + (i / 24) * innerW,
    });
  }

  const nowIdxFloor = Math.floor(nowIndex);
  const nowSlot = hasData && nowIdxFloor < N ? slots[nowIdxFloor] : null;
  const nowVal = nowSlot?.spot_sek ?? null;
  const nowX = xFor(nowIndex);
  const hoverCx = hover != null ? (xFor(hover) + xFor(hover + 1)) / 2 : 0;

  return (
    <svg ref={containerRef} className="energy-chart" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" style={{ height }}>
      <defs>
        <pattern id="resumeStripe" patternUnits="userSpaceOnUse" width="6" height="6" patternTransform="rotate(45)">
          <rect width="6" height="6" fill="transparent"/>
          <line x1="0" y1="0" x2="0" y2="6" stroke="var(--good)" strokeWidth="1.5" strokeOpacity="0.55"/>
        </pattern>
        <radialGradient id="nowGlow" cx="50%" cy="50%" r="50%">
          <stop offset="0%"   stopColor="var(--price-expensive)" stopOpacity="0.55"/>
          <stop offset="60%"  stopColor="var(--price-expensive)" stopOpacity="0.18"/>
          <stop offset="100%" stopColor="var(--price-expensive)" stopOpacity="0"/>
        </radialGradient>
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
          {resumeBand}
          {dhwBand}
          {slots.map((p, i) => {
            const v = p?.spot_sek;
            if (v == null) return null;
            const segs = [
              <line key={`h${i}`}
                    x1={xFor(i)}     y1={yFor(v)}
                    x2={xFor(i + 1)} y2={yFor(v)}
                    stroke={levelColor(p.level)}
                    strokeWidth="2"
                    strokeLinecap="butt"/>,
            ];
            const next = slots[i + 1];
            if (next?.spot_sek != null) {
              segs.push(
                <line key={`v${i}`}
                      x1={xFor(i + 1)} y1={yFor(v)}
                      x2={xFor(i + 1)} y2={yFor(next.spot_sek)}
                      stroke={levelColor((next.spot_sek > v ? next : p).level)}
                      strokeWidth="2"
                      strokeLinecap="butt"/>
              );
            }
            return segs;
          })}
        </>
      )}

      {/* now line — only when we have a real value to anchor to */}
      {hasData && nowVal != null && (() => {
        const priceTxt = `${nowVal.toFixed(2)} kr/kWh`;
        const lbl = `NOW · ${priceTxt}`;
        const tw = lbl.length * 5.8 + 8;
        const lx = Math.max(padL + tw/2, Math.min(w - padR - tw/2, nowX));
        return (
          <>
            <line className="now-line" x1={nowX} x2={nowX} y1={padT} y2={h - padB} />
            <rect className="now-label-bg" x={lx - tw/2} y={padT - 13} width={tw} height={12} rx="2"/>
            <text className="now-label" x={lx} y={padT - 4} textAnchor="middle">
              <tspan className="now-label-tag">NOW</tspan>
              <tspan dx="4" fill={levelColor(nowSlot.level)} style={{ textTransform: "none", letterSpacing: "0.02em" }}>{priceTxt}</tspan>
            </text>
            <circle cx={xFor(nowIdxFloor)} cy={yFor(nowVal)} r="14" fill="url(#nowGlow)" />
            <circle cx={xFor(nowIdxFloor)} cy={yFor(nowVal)} r="4"
                    fill={levelColor(nowSlot.level)}
                    stroke="var(--bg)" strokeWidth="1.5"/>
          </>
        );
      })()}

      {!hasData && (
        <text x={w / 2} y={h / 2} textAnchor="middle" dominantBaseline="middle"
              fill="var(--text-3)" fontSize="12" fontStyle="italic">
          No price data yet
        </text>
      )}

      <rect x={padL} y={padT} width={innerW} height={innerH}
        fill="transparent"
        style={{ touchAction: "none" }}
        onPointerDown={(e) => {
          if (N === 0) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          const rect = e.currentTarget.getBoundingClientRect();
          const ratio = (e.clientX - rect.left) / rect.width;
          const i = Math.max(0, Math.min(N - 1, Math.round(ratio * (N - 1))));
          setHover(i);
        }}
        onPointerMove={(e) => {
          if (N === 0) return;
          if (e.pointerType !== "mouse" && e.buttons === 0 && !e.currentTarget.hasPointerCapture?.(e.pointerId)) return;
          const rect = e.currentTarget.getBoundingClientRect();
          const ratio = (e.clientX - rect.left) / rect.width;
          const i = Math.max(0, Math.min(N - 1, Math.round(ratio * (N - 1))));
          if (i !== hover) setHover(i);
        }}
        onPointerUp={(e) => {
          if (e.pointerType !== "mouse") setHover(null);
        }}
        onPointerCancel={() => setHover(null)}
        onPointerLeave={(e) => {
          if (e.pointerType === "mouse") setHover(null);
        }}/>

      {hasData && hover != null && slots[hover]?.spot_sek != null && (
        <g pointerEvents="none">
          <line x1={hoverCx} x2={hoverCx} y1={padT} y2={h-padB}
                stroke="var(--text-3)" strokeDasharray="2 3" strokeWidth="1"/>
          <circle cx={hoverCx} cy={yFor(slots[hover].spot_sek)} r="5"
                  fill={levelColor(slots[hover].level)}
                  stroke="var(--bg)" strokeWidth="2"/>
          {(() => {
            const p = slots[hover];
            const tx = hoverCx;
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
});

window.EnergyChart = EnergyChart;
