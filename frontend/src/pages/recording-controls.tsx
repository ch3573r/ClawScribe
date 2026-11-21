import Head from 'next/head';
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { BookmarkPlus, Square } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Progress } from '@/components/ui/progress';
import { levelToDb, SegmentedLevelMeter } from '@/components/AudioLevelMeter';
import type { RecordingLevelsPayload, SourceLevelPayload } from '@/lib/recording-levels';
import { bookmarkTime } from '@/lib/library';
import { deviceLabel, recordingHealthMetrics, recordingHealthWarnings, type MissingRecordingDevice, type RecordingHealth } from '@/lib/recording-health';
import { applyAccent, applyThemePreference, DEFAULT_ACCENT_ID, subscribeToSystemTheme, type ThemePreference } from '@/lib/theme';

type Status =
  | { state: 'recording'; seconds: number; paused: boolean; levels: RecordingLevelsPayload | null }
  | { state: 'saving'; seconds: number; progress: number; message: string }
  | { state: 'review' | 'ended' };

type Snapshot = Status & { appearance: { theme: ThemePreference; accent: string | null } | null; health?: RecordingHealth | null };

function Level({ label, reading, paused }: { label: string; reading?: SourceLevelPayload | null; paused: boolean }) {
  return <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
    <span>{label}</span>
    {paused || reading === null
      ? <span>{paused ? 'Paused' : 'Not captured'}</span>
      : <SegmentedLevelMeter size="mini" label={label + ' level'} db={reading ? levelToDb(reading.rms) : null}
          peakDb={reading ? levelToDb(reading.peak) : null} />}
  </div>;
}

export default function FloatingControls() {
  const [status, setStatus] = useState<Snapshot | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState<'note' | 'stop' | 'pause' | null>(null);
  const [retrying, setRetrying] = useState(false);
  const retryInFlight = useRef(false);
  const [retryError, setRetryError] = useState('');
  const busy = useRef(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');

  const theme = status?.appearance?.theme ?? 'system';
  const accent = status?.appearance?.accent ?? DEFAULT_ACCENT_ID;
  useEffect(() => {
    const appearance = () => {
      applyThemePreference(theme);
      applyAccent(accent);
    };
    appearance();
    return subscribeToSystemTheme(appearance);
  }, [theme, accent]);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
    async function refresh() {
      let delay = 1000;
      try {
        const next = await invoke<Snapshot>('floating_status');
        if (disposed) return;
        setStatus(next);
        setUnavailable(false);
        if (next.state === 'ended') {
          await invoke('dismiss_floating_controls');
          return;
        }
        if (next.state === 'review') return;
        if (next.state === 'recording' && !next.paused) delay = reduced.matches ? 500 : 250;
      } catch {
        if (!disposed) {
          setStatus(null);
          setUnavailable(true);
        }
      }
      if (!disposed) timer = setTimeout(() => void refresh(), delay);
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);

  const recording = status?.state === 'recording' ? status : null;
  const saving = status?.state === 'saving' ? status : null;
  const health = status?.health;
  const warnings = health ? recordingHealthWarnings(health) : [];
  const metrics = health ? recordingHealthMetrics(health) : [];
  const canStop = !!recording && !unavailable && !pending;
  const canNote = canStop;
  const canRetry = canStop && !retrying && !health?.reconnecting;

  async function saveNote(event: FormEvent) {
    event.preventDefault();
    if (!canNote || busy.current) return;
    busy.current = true;
    setPending('note');
    setNotice('');
    setError('');
    const text = draft.trim();
    try {
      await invoke('floating_note', { label: text || 'Review this' });
      setDraft('');
      setNotice(text ? 'Note saved' : 'Bookmark saved');
    } catch {
      setError('Could not save this note. Check the recording and try again.');
    } finally {
      busy.current = false;
      setPending(null);
    }
  }

  async function stop() {
    if (!canStop || busy.current) return;
    busy.current = true;
    setPending('stop');
    setNotice('');
    setError('');
    try {
      await invoke('floating_stop');
    } catch {
      setError('Could not stop the recording. Try Stop again.');
    } finally {
      busy.current = false;
      setPending(null);
    }
  }

  async function setPaused() {
    if (!recording || !canNote || busy.current) return;
    busy.current = true;
    setPending('pause');
    setError('');
    try {
      await invoke('floating_set_paused', { paused: !recording.paused });
    } catch (error) {
      setError(typeof error === 'string' ? error : 'Could not change recording pause state.');
    } finally {
      busy.current = false;
      setPending(null);
    }
  }

  async function reconnect(device: MissingRecordingDevice) {
    if (!canRetry || retryInFlight.current) return;
    retryInFlight.current = true;
    setRetrying(true);
    setRetryError('');
    try {
      const result = await invoke<{ reconnected: boolean }>('floating_reconnect', { deviceName: device.name, deviceType: device.device_type });
      if (!result.reconnected) setRetryError('Audio device still unavailable. Check its connection and try again.');
    } catch (error) {
      setRetryError(typeof error === 'string' ? error : 'Could not reconnect the audio device.');
    } finally {
      retryInFlight.current = false;
      setRetrying(false);
    }
  }

  const stopping = !!saving || pending === 'stop';
  const title = saving ? 'Saving recording' : stopping ? 'Stopping recording'
    : recording ? (recording.paused ? 'Paused' : 'Recording')
    : status?.state === 'review' || status?.state === 'ended' ? 'Recording stopped'
    : unavailable ? 'Recording controls' : 'Loading controls…';
  const seconds = recording?.seconds ?? saving?.seconds;

  return <>
    <Head>
      <title>Recording controls</title>
      <meta httpEquiv="Content-Security-Policy" content="frame-src 'none'; object-src 'none'; media-src 'none'; img-src 'self'; form-action 'none'; connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost" />
    </Head>
    <main className="h-screen space-y-3 overflow-y-auto bg-background p-3 text-foreground" style={{ fontFamily: 'system-ui, sans-serif' }}>
      <header className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2 text-sm font-semibold">
            {recording && <span aria-hidden="true" className={`size-2 rounded-full ${recording.paused ? 'bg-warning-foreground' : 'bg-destructive'}`} />}
            {title}
          </h1>
          {seconds != null && <span role="timer" aria-label="Active recording time" className="text-lg font-medium tabular-nums">{bookmarkTime(seconds)}</span>}
        </div>
        {recording && <Button type="button" size="sm" variant="outline" disabled={!canNote} onClick={setPaused}>
          {recording.paused ? 'Resume' : 'Pause'}
        </Button>}
        {recording && <Button type="button" size="sm" variant="destructive" disabled={!canStop} onClick={stop}>
          <Square aria-hidden="true" className="fill-current" />Stop
        </Button>}
      </header>
      {health?.reconnecting && <p role="status" className="text-xs">Checking audio device…</p>}
      {warnings.length > 0 && <ul role="alert" className="space-y-1 text-xs text-warning-foreground">
        {warnings.map(warning => <li key={warning}>{warning}</li>)}
      </ul>}
      {recording && health?.disconnected_devices.map(device => <div key={device.device_type} className="flex items-center justify-between gap-2 text-xs">
        <span>{deviceLabel(device)} unavailable</span>
        <Button type="button" size="sm" variant="outline" disabled={!canRetry} onClick={() => void reconnect(device)}>Retry</Button>
      </div>)}
      {recording && !!health?.disconnected_devices.length && retryError && <p role="alert" className="text-xs text-destructive">{retryError}</p>}
      {saving ? <div className="space-y-2">
        <p className="text-sm" role="status">{saving.message}</p>
        <Progress value={saving.progress} aria-label="Recording save progress" aria-valuenow={saving.progress} />
      </div> : recording && <>
        <div role="group" aria-label="Recording levels" className="space-y-1.5 border-y border-border py-2">
          <Level label="Microphone" paused={recording.paused} reading={recording.levels?.mic} />
          <Level label="System audio" paused={recording.paused} reading={recording.levels?.system} />
        </div>
        <form onSubmit={saveNote} className="flex items-center gap-2">
          <Input aria-label="Optional timestamped note" placeholder="Optional note" maxLength={160} autoComplete="off"
            className="min-w-0 flex-1" value={draft} readOnly={!canNote} onChange={e => setDraft(e.target.value)} />
          <Button size="sm" type="submit" variant="outline" disabled={!canNote}>
            <BookmarkPlus aria-hidden="true" />{draft.trim() ? 'Save note' : 'Bookmark'}
          </Button>
        </form>
      </>}
      {metrics.length > 0 && <p className="text-xs text-muted-foreground">{metrics.join(' · ')}</p>}
      {status?.state === 'review' && <p role="alert" className="text-sm">Open ClawScribe to review the recording and its recovery options.</p>}
      {unavailable || (canStop && error) ? <p role="alert" className="text-xs text-destructive">{unavailable ? 'Recording status unavailable. Reconnecting…' : error}</p>
        : <p role="status" className="text-xs text-muted-foreground">{recording && notice}</p>}
    </main>
  </>;
}
