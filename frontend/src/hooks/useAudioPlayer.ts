import { useState, useEffect, useRef, useCallback, useMemo, useSyncExternalStore } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { DEFAULT_PLAYBACK_RATE, PLAYBACK_RATE_STORAGE_KEY, normalizePlaybackRate } from '@/lib/playback-controls';
import { resumablePlaybackPosition } from './transcript-playback';

const POSITION_WRITE_INTERVAL_MS = 5_000;
const POSITION_STORAGE_KEY = 'clawscribe.playbackPositions';
const MAX_SAVED_POSITIONS = 200;
type SavedPosition = [meetingId: string, seconds: number];

function readPositions(storage: Storage): SavedPosition[] {
  try {
    const saved: unknown = JSON.parse(storage.getItem(POSITION_STORAGE_KEY) ?? '[]');
    if (!Array.isArray(saved)) return [];
    return saved.filter((entry): entry is SavedPosition =>
      Array.isArray(entry) && entry.length === 2 && typeof entry[0] === 'string' &&
      typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] >= 0
    ).slice(0, MAX_SAVED_POSITIONS);
  } catch {
    return [];
  }
}

function writePosition(storage: Storage, meetingId: string, seconds: number, positions = readPositions(storage)) {
  const remaining = positions.filter(([id]) => id !== meetingId).slice(0, MAX_SAVED_POSITIONS - 1);
  storage.setItem(POSITION_STORAGE_KEY, JSON.stringify([[meetingId, seconds], ...remaining]));
  storage.removeItem(`clawscribe.playbackPosition.${meetingId}`);
}

function readPosition(meetingId: string): number {
  try {
    const storage = window.localStorage;
    const positions = readPositions(storage);
    const saved = positions.find(([id]) => id === meetingId);
    if (saved) return saved[1];
    const legacy = storage.getItem(`clawscribe.playbackPosition.${meetingId}`);
    if (legacy === null) return 0;
    const seconds = Number(legacy);
    if (!Number.isFinite(seconds) || seconds < 0) return 0;
    try { writePosition(storage, meetingId, seconds, positions); } catch { /* Keep the legacy entry if migration fails. */ }
    return seconds;
  } catch {
    return 0;
  }
}

// The chosen speed is a per-viewer convenience; the player works without storage.
function storedRate(): number {
  try { return typeof window !== 'undefined' ? normalizePlaybackRate(window.localStorage?.getItem(PLAYBACK_RATE_STORAGE_KEY) ?? DEFAULT_PLAYBACK_RATE) : DEFAULT_PLAYBACK_RATE; }
  catch { return DEFAULT_PLAYBACK_RATE; }
}

export type PlaybackClock = {
  getSnapshot: () => number;
  subscribe: (listener: () => void) => () => void;
};

const noPlaybackTime = () => undefined;
const noSubscription = () => () => {};

export function usePlaybackTime(clock?: PlaybackClock) {
  return useSyncExternalStore(clock?.subscribe ?? noSubscription, clock?.getSnapshot ?? noPlaybackTime, noPlaybackTime);
}

function createPlaybackClock() {
  let seconds = 0;
  const listeners = new Set<() => void>();
  const clock: PlaybackClock = {
    getSnapshot: () => seconds,
    subscribe: listener => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
  };
  const setTime = (next: number) => {
    if (Object.is(next, seconds)) return;
    seconds = next;
    listeners.forEach(listener => listener());
  };
  return { clock, setTime };
}

export const useAudioPlayer = (audioPath: string | null, meetingId?: string) => {
  const [isPlaying, setIsPlaying] = useState(false);
  const [{ clock, setTime }] = useState(createPlaybackClock);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const pendingSeek = useRef<number | null>(null);
  const [rate, setRateState] = useState(storedRate);
  const rateRef = useRef(rate);
  const [volume, setVolumeState] = useState(1);
  const volumeRef = useRef(volume);
  const [muted, setMuted] = useState(false);
  const mutedRef = useRef(muted);

  useEffect(() => {
    setIsPlaying(false); setTime(0); setDuration(0); setError(null);
    pendingSeek.current = null;
    if (!audioPath) return;

    // The backend grants access to this resolved meeting file. The asset
    // protocol serves byte ranges; the webview decodes only buffered media.
    const audio = new window.Audio();
    audioRef.current = audio;
    audio.preload = 'metadata';
    audio.playbackRate = rateRef.current;
    audio.preservesPitch = true;
    audio.volume = volumeRef.current;
    audio.muted = mutedRef.current;
    let positionReady = false;
    let savedSeconds = 0;
    let lastWriteAt = Date.now();
    const savePosition = () => {
      if (!meetingId || !positionReady || !Number.isFinite(audio.currentTime)) return;
      const seconds = audio.currentTime;
      if (savedSeconds === seconds) return;
      lastWriteAt = Date.now();
      try {
        writePosition(window.localStorage, meetingId, seconds);
        savedSeconds = seconds;
      } catch { /* Playback works without storage. */ }
    };
    const metadata = () => {
      if (!Number.isFinite(audio.duration) || audio.duration <= 0) return;
      setDuration(audio.duration);
      if (positionReady) return;
      positionReady = true;
      if (meetingId) savedSeconds = readPosition(meetingId);
      const position = pendingSeek.current === null
        ? resumablePlaybackPosition(savedSeconds, audio.duration)
        : Math.min(pendingSeek.current, audio.duration);
      pendingSeek.current = null;
      if (position !== audio.currentTime) audio.currentTime = position;
      setTime(audio.currentTime);
    };
    const time = () => {
      setTime(audio.currentTime);
      if (meetingId && Date.now() - lastWriteAt >= POSITION_WRITE_INTERVAL_MS) savePosition();
    };
    const playing = () => { setIsPlaying(true); setError(null); };
    const paused = () => { setIsPlaying(false); savePosition(); };
    const ended = () => { setIsPlaying(false); audio.currentTime = 0; setTime(0); savePosition(); };
    const failed = () => { setIsPlaying(false); setError('Failed to load audio file'); };
    const handlers = { loadedmetadata: metadata, durationchange: metadata, timeupdate: time, playing, pause: paused, ended, error: failed };
    for (const [event, handler] of Object.entries(handlers)) audio.addEventListener(event, handler);
    if (meetingId) window.addEventListener('pagehide', savePosition);
    audio.src = convertFileSrc(audioPath);
    audio.load();
    return () => {
      if (meetingId) window.removeEventListener('pagehide', savePosition);
      savePosition();
      for (const [event, handler] of Object.entries(handlers)) audio.removeEventListener(event, handler);
      audioRef.current = null;
      audio.pause();
      audio.removeAttribute('src');
      audio.load();
    };
  }, [audioPath, meetingId, setTime]);

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
    setTime(audio.currentTime);
  }, [setTime]);

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

  const setVolume = useCallback((value: number) => {
    if (!Number.isFinite(value)) return;
    const next = Math.max(0, Math.min(1, value));
    volumeRef.current = next;
    if (audioRef.current) audioRef.current.volume = next;
    setVolumeState(next);
  }, []);

  const toggleMute = useCallback(() => {
    const next = !mutedRef.current;
    mutedRef.current = next;
    if (audioRef.current) audioRef.current.muted = next;
    setMuted(next);
  }, []);

  return useMemo(() => ({ isPlaying, clock, duration, error, rate, volume, muted, play, pause, seek, seekBy, setRate, setVolume, toggleMute }),
    [isPlaying, clock, duration, error, rate, volume, muted, play, pause, seek, seekBy, setRate, setVolume, toggleMute]);
};
