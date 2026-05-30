// `VolumeSlider` — audio volume / mute control.
//
// Renders a `0–100` integer slider plus a mute toggle. Both controls are
// fully controlled by the parent: this component only emits events.

import type { ChangeEvent, ReactElement } from 'react';

export interface VolumeSliderProps {
  /** Current volume in `0.0..=1.0`. */
  volume: number;
  /** Whether the audio output is muted. */
  muted: boolean;
  /** Invoked with the new volume in `0.0..=1.0`. */
  onVolumeChange: (volume: number) => void;
  /** Invoked when the user toggles the mute checkbox. */
  onMutedChange: (muted: boolean) => void;
}

export function VolumeSlider({
  volume,
  muted,
  onVolumeChange,
  onMutedChange,
}: VolumeSliderProps): ReactElement {
  const percent = Math.round(volume * 100);

  const handleSlider = (e: ChangeEvent<HTMLInputElement>): void => {
    const raw = Number(e.currentTarget.value);
    if (!Number.isFinite(raw)) return;
    const clamped = Math.min(100, Math.max(0, raw));
    onVolumeChange(clamped / 100);
  };

  const handleMute = (e: ChangeEvent<HTMLInputElement>): void => {
    onMutedChange(e.currentTarget.checked);
  };

  return (
    <div className="volume-slider">
      <label className="volume-slider__mute">
        <input type="checkbox" checked={muted} onChange={handleMute} />
        <span>Mute</span>
      </label>
      <label className="volume-slider__label" htmlFor="volume-slider-input">
        Volume
      </label>
      <input
        id="volume-slider-input"
        type="range"
        min={0}
        max={100}
        step={1}
        value={percent}
        onChange={handleSlider}
        disabled={muted}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      />
      <span className="volume-slider__value" aria-live="polite">
        {percent}%
      </span>
    </div>
  );
}
