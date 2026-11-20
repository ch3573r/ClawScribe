import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { DEFAULT_PLAYBACK_RATE, PLAYBACK_RATE_STORAGE_KEY, normalizePlaybackRate } from '@/lib/playback-controls';

// The chosen speed is a per-viewer convenience; the player works without storage.
function storedRate(): number {
  try { return typeof window !== 'undefined' ? normalizePlaybackRate(window.localStorage?.getItem(PLAYBACK_RATE_STORAGE_KEY) ?? DEFAULT_PLAYBACK_RATE) : DEFAULT_PLAYBACK_RATE; }
  catch { return DEFAULT_PLAYBACK_RATE; }
}

export const useAudioPlayer = (audioPath: string | null) => {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const pendingSeek = useRef<number | null>(null);
  const [rate, setRateState] = useState(storedRate);
  const rateRef = useRef(rate);

  useEffect(() => {
    setIsPlaying(false); setCurrentTime(0); setDuration(0); setError(null);
    pendingSeek.current = null;
    if (!audioPath) return;

    // The backend grants access to this resolved meeting file. The asset
    // protocol serves byte ranges; the webview decodes only buffered media.
    const audio = new window.Audio();
    audioRef.current = audio;
    audio.preload = 'metadata';
    audio.playbackRate = rateRef.current;
    audio.preservesPitch = true;
    const metadata = () => {
      if (Number.isFinite(audio.duration)) {
        setDuration(audio.duration);
        if (pendingSeek.current !== null) {
          audio.currentTime = Math.min(pendingSeek.current, audio.duration);
          pendingSeek.current = null;
          setCurrentTime(audio.currentTime);
        }
      }
    };
    const time = () => setCurrentTime(audio.currentTime);
    const playing = () => { setIsPlaying(true); setError(null); };
    const paused = () => setIsPlaying(false);
    const ended = () => { setIsPlaying(false); audio.currentTime = 0; setCurrentTime(0); };
    const failed = () => { setIsPlaying(false); setError('Failed to load audio file'); };
    const handlers = { loadedmetadata: metadata, durationchange: metadata, timeupdate: time, playing, pause: paused, ended, error: failed };
    for (const [event, handler] of Object.entries(handlers)) audio.addEventListener(event, handler);
    audio.src = convertFileSrc(audioPath);
    audio.load();
    return () => {
      for (const [event, handler] of Object.entries(handlers)) audio.removeEventListener(event, handler);
      audioRef.current = null;
      audio.pause();
      audio.removeAttribute('src');
      audio.load();
    };
  }, [audioPath]);

  const play = useCallback(async () => {
    const audio = audioRef.current;
    if (!audio) return false;
    try {
      await audio.play();
      if (audioRef.current === audio) setError(null);
      return audioRef.current === audio;
    } catch {
      if (audioRef.current === audio) setError('Failed to play audio');
      return false;
    }
  }, []);

  const pause = useCallback(() => { audioRef.current?.pause(); }, []);

  const seek = useCallback(async (seconds: number) => {
    const audio = audioRef.current;
    if (!audio || !Number.isFinite(seconds)) return;
    const time = Math.max(0, seconds);
    if (!Number.isFinite(audio.duration)) { pendingSeek.current = time; return; }
    audio.currentTime = Math.min(time, audio.duration);
    setCurrentTime(audio.currentTime);
  }, []);

  // Relative to the element's own clock, so repeated key presses never use a stale time.
  const seekBy = useCallback(async (delta: number) => {
    const audio = audioRef.current;
    if (!audio || !Number.isFinite(delta)) return;
    await seek((pendingSeek.current ?? audio.currentTime) + delta);
  }, [seek]);

  const setRate = useCallback((value: number) => {
    const next = normalizePlaybackRate(value);
    rateRef.current = next;
    setRateState(next);
    if (audioRef.current) audioRef.current.playbackRate = next;
    try { window.localStorage?.setItem(PLAYBACK_RATE_STORAGE_KEY, String(next)); } catch { /* not remembered */ }
  }, []);

  return useMemo(() => ({ isPlaying, currentTime, duration, error, rate, play, pause, seek, seekBy, setRate }),
    [isPlaying, currentTime, duration, error, rate, play, pause, seek, seekBy, setRate]);
};
