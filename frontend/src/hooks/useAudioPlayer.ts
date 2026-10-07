import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';

export const useAudioPlayer = (audioPath: string | null) => {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const pendingSeek = useRef<number | null>(null);

  useEffect(() => {
    setIsPlaying(false); setCurrentTime(0); setDuration(0); setError(null);
    pendingSeek.current = null;
    if (!audioPath) return;

    // The backend grants access to this resolved meeting file. The asset
    // protocol serves byte ranges; the webview decodes only buffered media.
    const audio = new window.Audio();
    audioRef.current = audio;
    audio.preload = 'metadata';
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

  return useMemo(() => ({ isPlaying, currentTime, duration, error, play, pause, seek }),
    [isPlaying, currentTime, duration, error, play, pause, seek]);
};
