import React from 'react';

interface AudioLevelMeterProps {
  rmsLevel: number;    // 0.0 to 1.0
  peakLevel: number;   // 0.0 to 1.0
  isActive: boolean;   // Whether audio is being detected
  deviceName: string;
  className?: string;
  size?: 'small' | 'medium' | 'large';
}

export function AudioLevelMeter({
  rmsLevel,
  peakLevel,
  isActive,
  deviceName,
  className = '',
  size = 'medium'
}: AudioLevelMeterProps) {
  // Normalize levels to 0-1 range and apply log scaling for better visual representation
  const normalizedRms = Math.max(0, Math.min(1, rmsLevel));
  const normalizedPeak = Math.max(0, Math.min(1, peakLevel));

  // Apply logarithmic scaling for better visual representation of audio levels
  const logRms = normalizedRms > 0 ? Math.log10(normalizedRms * 9 + 1) : 0;
  const logPeak = normalizedPeak > 0 ? Math.log10(normalizedPeak * 9 + 1) : 0;

  // Calculate percentages for display
  const rmsPercent = Math.round(logRms * 100);
  const peakPercent = Math.round(logPeak * 100);

  // Color coding based on level
  const getLevelColor = (level: number) => {
    if (level < 0.3) return 'bg-success-foreground';
    if (level < 0.7) return 'bg-warning-foreground';
    return 'bg-error-foreground';
  };

  const rmsColor = getLevelColor(logRms);
  const peakColor = getLevelColor(logPeak);

  // Size variants
  const sizeClasses = {
    small: {
      container: 'h-2',
      text: 'text-xs',
      meter: 'h-1.5'
    },
    medium: {
      container: 'h-3',
      text: 'text-sm',
      meter: 'h-2'
    },
    large: {
      container: 'h-4',
      text: 'text-base',
      meter: 'h-3'
    }
  };

  const sizes = sizeClasses[size];

  return (
    <div className={`flex items-center space-x-2 ${className}`}>
      {/* Device activity indicator */}
      <div className={`w-2 h-2 rounded-full ${
        isActive ? 'bg-success-foreground animate-pulse' : 'bg-secondary'
      }`} title={`${deviceName} - ${isActive ? 'Active' : 'Inactive'}`} />

      {/* Level meter container */}
      <div className={`flex-1 ${sizes.container} relative`}>
        {/* Background */}
        <div className="w-full h-full bg-secondary rounded-sm overflow-hidden">
          {/* RMS level bar (main level) */}
          <div
            className={`${sizes.meter} ${rmsColor} transition-all duration-150 ease-out rounded-sm`}
            style={{ width: `${rmsPercent}%` }}
          />

          {/* Peak level indicator (thin line) */}
          {peakPercent > rmsPercent && (
            <div
              className={`absolute top-0 bottom-0 w-0.5 ${peakColor} transition-all duration-75`}
              style={{ left: `${peakPercent}%` }}
            />
          )}
        </div>

        {/* Level markers */}
        <div className="absolute inset-0 flex justify-between items-center px-1 pointer-events-none">
          {/* 25% marker */}
          <div className="w-px h-full bg-muted opacity-30" style={{ marginLeft: '25%' }} />
          {/* 50% marker */}
          <div className="w-px h-full bg-muted opacity-30" style={{ marginLeft: '50%' }} />
          {/* 75% marker */}
          <div className="w-px h-full bg-muted opacity-30" style={{ marginLeft: '75%' }} />
        </div>
      </div>

      {/* Level percentage display */}
      <div className={`${sizes.text} text-muted-foreground font-mono min-w-[3rem] text-right`}>
        {rmsPercent}%
      </div>
    </div>
  );
}

interface CompactAudioLevelMeterProps {
  rmsLevel: number;
  peakLevel: number;
  isActive: boolean;
  className?: string;
}

// Compact version for inline display in dropdowns
export function CompactAudioLevelMeter({
  rmsLevel,
  peakLevel,
  isActive,
  className = ''
}: CompactAudioLevelMeterProps) {
  const normalizedRms = Math.max(0, Math.min(1, rmsLevel));
  const logRms = normalizedRms > 0 ? Math.log10(normalizedRms * 9 + 1) : 0;
  const rmsPercent = Math.round(logRms * 100);

  const getLevelColor = (level: number) => {
    if (level < 0.3) return 'bg-success-foreground';
    if (level < 0.7) return 'bg-warning-foreground';
    return 'bg-error-foreground';
  };

  return (
    <div className={`flex items-center space-x-1 ${className}`}>
      {/* Activity dot */}
      <div className={`w-1.5 h-1.5 rounded-full ${
        isActive ? 'bg-success-foreground' : 'bg-secondary'
      }`} />

      {/* Mini meter */}
      <div className="w-8 h-1.5 bg-secondary rounded-sm overflow-hidden">
        <div
          className={`h-full ${getLevelColor(logRms)} transition-all duration-150`}
          style={{ width: `${rmsPercent}%` }}
        />
      </div>
    </div>
  );
}

/** Quietest level the segmented meter can show. */
export const LEVEL_FLOOR_DB = -60;

/** Linear amplitude (0 to 1) as decibels relative to full scale, clamped to the meter range. */
export function levelToDb(level: number): number {
  if (!Number.isFinite(level) || level <= 0) return LEVEL_FLOOR_DB;
  return Math.max(LEVEL_FLOOR_DB, Math.min(0, 20 * Math.log10(level)));
}

/** Position on the meter, 0 at the floor and 1 at full scale. */
export function dbToFraction(db: number): number {
  return Math.max(0, Math.min(1, (db - LEVEL_FLOOR_DB) / -LEVEL_FLOOR_DB));
}

export const LEVEL_TICKS_DB = [-60, -40, -24, -12, -6, 0] as const;

interface SegmentedLevelMeterProps {
  /** Current level in dB relative to full scale. `null` draws an empty meter. */
  db: number | null;
  /** Held peak in dB, drawn as a single bright segment above the level. */
  peakDb?: number | null;
  segments?: number;
  /** `large` is the signal check meter, `mini` sits inline in a settings row. */
  size?: 'large' | 'mini';
  label: string;
  className?: string;
}

/**
 * Segmented level meter. Segments are blue up to -12 dB, amber up to -3 dB and
 * red above, matching how loud a microphone should be for clean transcription.
 */
export function SegmentedLevelMeter({
  db,
  peakDb = null,
  segments,
  size = 'large',
  label,
  className = '',
}: SegmentedLevelMeterProps) {
  const count = segments ?? (size === 'large' ? 44 : 24);
  const lit = db === null ? 0 : Math.round(dbToFraction(db) * count);
  const peak = peakDb === null || db === null ? 0 : Math.round(dbToFraction(peakDb) * count);
  const peakIndex = peak > lit ? peak - 1 : -1;
  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuemin={LEVEL_FLOOR_DB}
      aria-valuemax={0}
      aria-valuenow={db === null ? LEVEL_FLOOR_DB : Math.round(db)}
      aria-valuetext={db === null ? 'Not measured' : `${Math.round(db)} dB`}
      className={`grid ${size === 'large' ? 'h-7 gap-0.5' : 'h-3 w-[150px] shrink-0 gap-0.5'} ${className}`}
      style={{ gridTemplateColumns: `repeat(${count}, minmax(0, 1fr))` }}
    >
      {Array.from({ length: count }, (_, index) => {
        const segmentDb = LEVEL_FLOOR_DB + ((index + 1) / count) * -LEVEL_FLOOR_DB;
        const tone = segmentDb > -3 ? 'bg-error-foreground' : segmentDb > -12 ? 'bg-warning-foreground' : 'bg-primary';
        const color = index < lit ? tone : index === peakIndex ? 'bg-foreground' : 'bg-muted';
        return <i key={index} aria-hidden="true" className={`rounded-[1.5px] ${color}`} />;
      })}
    </div>
  );
}

/** Scale under a large meter, placed at true dB positions. */
export function LevelTicks() {
  return (
    <div aria-hidden="true" className="relative mt-2 h-3 font-mono text-[10px] text-muted-foreground">
      {LEVEL_TICKS_DB.map((tick, index) => (
        <span
          key={tick}
          className="absolute top-0"
          style={{
            left: `${dbToFraction(tick) * 100}%`,
            transform: `translateX(${index === 0 ? '0' : index === LEVEL_TICKS_DB.length - 1 ? '-100%' : '-50%'})`,
          }}
        >
          {tick}
        </span>
      ))}
    </div>
  );
}
