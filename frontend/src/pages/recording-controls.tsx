import Head from 'next/head';
import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent, type MouseEvent, type Ref, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { BookmarkPlus, ExternalLink, Mic, MonitorSpeaker, Pause, Play, Square, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Progress } from '@/components/ui/progress';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { levelToDb, SegmentedLevelMeter } from '@/components/AudioLevelMeter';
import type { RecordingLevelsPayload, SourceLevelPayload } from '@/lib/recording-levels';
import { bookmarkTime } from '@/lib/library';
import { deviceLabel, recordingHealthWarnings, type MissingRecordingDevice, type RecordingHealth } from '@/lib/recording-health';
import { applyAccent, applyThemePreference, DEFAULT_ACCENT_ID, subscribeToSystemTheme, type ThemePreference } from '@/lib/theme';
import { shortcutHint, type ShortcutAction } from '@/lib/shortcuts';
import { drainEta } from '@/lib/recording-progress';

type Status =
  | { state: 'recording'; seconds: number; paused: boolean; levels: RecordingLevelsPayload | null; mode: 'live' | 'audio_only'; mic_no_signal: boolean }
  | { state: 'saving'; seconds: number; progress: number; message: string; estimated_seconds_remaining?: number | null }
  | { state: 'review' | 'ended' };

type Snapshot = Status & {
  appearance: { theme: ThemePreference; accent: string | null } | null;
  health?: RecordingHealth | null;
  shortcuts?: [ShortcutAction, string][];
};

function Level({ label, reading, paused }: { label: string; reading?: SourceLevelPayload | null; paused: boolean }) {
  const Icon = label === 'Microphone' ? Mic : MonitorSpeaker;
  const inactive = paused ? 'Paused' : reading === null ? 'Not captured' : null;
  return <div title={inactive ? label + ': ' + inactive : label} className="flex h-4 items-center gap-1.5 text-xs text-muted-foreground">
    <Icon aria-hidden="true" className="size-3 shrink-0" />
    {inactive
      ? <span aria-label={label + ': ' + inactive} className="whitespace-nowrap text-[10px] leading-none">{inactive}</span>
      : <SegmentedLevelMeter size="mini" segments={9} className="!h-1.5 !w-14" label={label + ' level'}
          db={reading ? levelToDb(reading.rms) : null} peakDb={reading ? levelToDb(reading.peak) : null} />}
  </div>;
}

function Control({ label, children, onClick, disabled, destructive, buttonRef, expanded, shortcut }: {
  label: string;
  children: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
  buttonRef?: Ref<HTMLButtonElement>;
  expanded?: boolean;
  shortcut?: string;
}) {
  return <Tooltip>
    <TooltipTrigger asChild>
      <Button ref={buttonRef} type="button" size="icon" variant={destructive ? 'destructive' : 'ghost'}
        className="size-8 shrink-0" onClick={onClick} disabled={disabled} aria-label={label}
        aria-expanded={expanded} aria-keyshortcuts={shortcut}>{children}</Button>
    </TooltipTrigger>
    <TooltipContent side="left" sideOffset={4} collisionPadding={2} className="pointer-events-none px-2 py-0.5">
      {shortcutHint(label, shortcut)}
    </TooltipContent>
  </Tooltip>;
}

export default function FloatingControls() {
  const [status, setStatus] = useState<Snapshot | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [draft, setDraft] = useState('');
  const [noteOpen, setNoteOpen] = useState(false);
  const noteButton = useRef<HTMLButtonElement>(null);
  const noteInput = useRef<HTMLInputElement>(null);
  const returnNoteFocus = useRef(false);
  const panel = useRef<HTMLDivElement>(null);
  const [pending, setPending] = useState<'note' | 'stop' | 'pause' | null>(null);
  const [retrying, setRetrying] = useState(false);
  const retryInFlight = useRef(false);
  const [retryError, setRetryError] = useState('');
  const busy = useRef(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [openingMain, setOpeningMain] = useState(false);
  const opening = useRef(false);

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
    const content = panel.current;
    if (!content) return;
    // Measure the content independently of the native viewport being resized.
    const observer = new ResizeObserver(() => {
      void invoke('floating_resize', { height: Math.ceil(content.getBoundingClientRect().height) }).catch(() => {});
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(''), 1600);
    return () => clearTimeout(timer);
  }, [notice]);

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
  const eta = drainEta(saving?.estimated_seconds_remaining);
  const shortcuts = Object.fromEntries(status?.shortcuts ?? []);
  const health = recording ? status?.health : null;
  const warnings = health ? recordingHealthWarnings(health) : [];
  const canStop = !!recording && !unavailable && !pending;
  const canNote = canStop;
  const canRetry = canStop && !retrying && !health?.reconnecting;

  useEffect(() => {
    if (noteOpen) {
      noteInput.current?.focus();
    } else if (canNote && returnNoteFocus.current) {
      returnNoteFocus.current = false;
      noteButton.current?.focus();
    }
  }, [noteOpen, canNote]);

  useEffect(() => {
    if (noteOpen || !canNote) return;
    function openNote(event: globalThis.KeyboardEvent) {
      if (event.key.toLowerCase() === 'n' && !event.altKey && !event.ctrlKey && !event.metaKey && !event.isComposing) {
        event.preventDefault();
        setNoteOpen(true);
      }
    }
    window.addEventListener('keydown', openNote);
    return () => window.removeEventListener('keydown', openNote);
  }, [noteOpen, canNote]);

  function closeNote() {
    returnNoteFocus.current = true;
    setNoteOpen(false);
  }

  function handleKeys(event: KeyboardEvent<HTMLElement>) {
    if (event.key === 'Escape' && noteOpen) {
      event.preventDefault();
      closeNote();
    }
  }

  function drag(event: MouseEvent<HTMLElement>) {
    if (event.button === 0 && !(event.target as Element).closest('[data-no-drag]')) {
      void invoke('floating_drag').catch(() => {});
    }
  }

  async function dismiss() {
    try {
      await invoke('dismiss_floating_controls');
    } catch {
      setError('Could not close controls. Press Alt+F4 to close the window.');
    }
  }

  async function openMain() {
    if (opening.current) return;
    opening.current = true;
    setOpeningMain(true);
    setError('');
    try {
      await invoke('floating_open_main');
    } catch {
      setError('Could not open ClawScribe. Try again.');
    } finally {
      opening.current = false;
      setOpeningMain(false);
    }
  }

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
      closeNote();
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
  const review = status?.state === 'review';
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
    <main className="floating-controls h-dvh overflow-y-auto" aria-label="Recording controls" onKeyDown={handleKeys}>
      <div ref={panel} className="p-1.5">
        <TooltipProvider delayDuration={300}>
          <div className="rounded-xl border border-border bg-card text-card-foreground shadow-md shadow-foreground/15">
            <header className="flex h-12 select-none items-center gap-2.5 px-2.5" onMouseDown={drag}>
              {recording && !stopping && <span aria-hidden="true" title={title}
                className={`size-2 shrink-0 rounded-full ${recording.paused ? 'bg-warning-foreground' : 'bg-destructive motion-safe:animate-pulse'}`} />}
              <h1 role="status" className={recording && !stopping ? 'sr-only' : 'text-sm font-semibold'}>{title}</h1>
              {seconds != null && <div className="shrink-0 leading-tight">
                <span role="timer" aria-label="Active recording time" className="block text-base font-semibold tabular-nums">{bookmarkTime(seconds)}</span>
                {recording && <span className="inline-flex whitespace-nowrap rounded-full bg-muted px-1.5 py-0.5 text-[10px] font-medium leading-none text-muted-foreground">
                  {recording.mode === 'audio_only' ? 'Audio only' : 'Live'}
                </span>}
              </div>}
              <div className="min-w-0 flex-1">
                {recording && !stopping && !notice && <div role="group" aria-label="Recording levels">
                  <Level label="Microphone" paused={recording.paused} reading={recording.levels?.mic} />
                  <Level label="System audio" paused={recording.paused} reading={recording.levels?.system} />
                </div>}
                <div role="status" className="text-xs leading-tight text-muted-foreground">
                  {recording && !stopping && notice}
                </div>
              </div>
              <div data-no-drag className="flex shrink-0 items-center gap-1">
                {recording && !stopping && <>
                  <Control label={recording.paused ? 'Resume recording' : 'Pause recording'} disabled={!canNote} onClick={setPaused} shortcut={shortcuts.pauseResume}>
                    {recording.paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
                  </Control>
                  <Control label="Add note" buttonRef={noteButton} expanded={noteOpen} shortcut="N" disabled={!canNote}
                    onClick={() => noteOpen ? closeNote() : setNoteOpen(true)}><BookmarkPlus aria-hidden="true" /></Control>
                  <Control label="Stop recording" destructive disabled={!canStop} onClick={stop} shortcut={shortcuts.startStop}>
                    <Square aria-hidden="true" className="fill-current" />
                  </Control>
                  <Control label="Open main window" disabled={openingMain} onClick={openMain}><ExternalLink aria-hidden="true" /></Control>
                </>}
                {!review && <Control label="Close controls" onClick={dismiss}><X aria-hidden="true" /></Control>}
              </div>
            </header>
            {recording && noteOpen && !stopping && <form onSubmit={saveNote} className="flex items-center gap-2 border-t border-border p-2">
              <Input ref={noteInput} aria-label="Optional timestamped note" placeholder="Optional note" maxLength={160} autoComplete="off"
                className="h-8 min-w-0 flex-1" value={draft} readOnly={!canNote} onChange={e => setDraft(e.target.value)} />
              <Button className="h-8" size="sm" type="submit" variant="outline" disabled={!canNote}
                title={!draft.trim() ? shortcutHint('Bookmark', shortcuts.bookmarkRecording) : undefined}>
                {draft.trim() ? 'Save note' : 'Bookmark'}
              </Button>
            </form>}
            {recording && !stopping && !recording.paused && recording.mic_no_signal &&
              <p role="status" className="border-t border-border px-3 py-2 text-xs text-warning-foreground">No microphone signal yet</p>}
            {(health?.reconnecting || warnings.length > 0 || !!health?.disconnected_devices.length) &&
              <div className="space-y-1.5 border-t border-border px-3 py-2 text-xs">
                {health?.reconnecting && <p role="status">Checking audio device…</p>}
                {warnings.length > 0 && <ul role="alert" className="space-y-1 break-words text-warning-foreground">
                  {warnings.map(warning => <li key={warning}>{warning}</li>)}
                </ul>}
                {health?.disconnected_devices.map(device => <div key={device.device_type} className="flex items-center justify-between gap-2">
                  <span>{deviceLabel(device)} unavailable</span>
                  <Button type="button" size="sm" variant="outline" disabled={!canRetry} onClick={() => void reconnect(device)}>Retry</Button>
                </div>)}
                {!!health?.disconnected_devices.length && retryError && <p role="alert" className="text-destructive">{retryError}</p>}
              </div>}
            {saving && <div className="space-y-2 border-t border-border px-3 py-2.5">
              <p className="text-xs" role="status">{saving.message}</p>
              {eta && <p className="text-xs text-muted-foreground">{eta}</p>}
              <Progress value={saving.progress} aria-label="Recording save progress" aria-valuenow={saving.progress} />
            </div>}
            {review && <div className="space-y-2 border-t border-border px-3 py-2.5">
              <p className="text-sm" role="alert">This recording needs review.</p>
              <div className="flex gap-2">
                <Button type="button" size="sm" disabled={openingMain} onClick={openMain}>Open ClawScribe</Button>
                <Button type="button" size="sm" variant="outline" onClick={dismiss}>Dismiss</Button>
              </div>
            </div>}
            {(unavailable || error) && <p role="alert" className="border-t border-border px-3 py-2 text-xs text-destructive">
              {unavailable ? 'Recording status unavailable. Reconnecting…' : error}
            </p>}
          </div>
        </TooltipProvider>
      </div>
    </main>
  </>;
}
