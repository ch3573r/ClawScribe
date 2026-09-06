import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { MeetingChat } from '@/components/MeetingDetails/MeetingChat';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';
import { MeetingNavigationItem } from '@/components/Sidebar/MeetingNavigationItem';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TooltipProvider } from '@/components/ui/tooltip';
import { useCompactLayout } from '@/hooks/useCompactLayout';
import { RecordingStateProvider, useRecordingState, useRecordingClock } from '@/contexts/RecordingStateContext';

function LifecycleProbe() {
  const state = useRecordingState();
  const fixture = (window as any).recordingFixture;
  fixture.lifecycleRenders++;
  return <span hidden data-testid="lifecycle-state">{state.status}:{String(state.isPaused)}</span>;
}
function ClockProbe() {
  const clock = useRecordingClock();
  return <span hidden data-testid="clock-value">{clock.activeDuration ?? 'none'}</span>;
}
function Fixture() {
  const [meetingId, setMeetingId] = useState('meeting-one');
  const [deleting, setDeleting] = useState(false);
  const [deleted, setDeleted] = useState(false);
  const [segments, setSegments] = useState(() => Array.from({length:3000}, (_,i) => ({id:`row-${i}`,timestamp:i*3,text:`Segment ${i}. Er kommt um 10 Uhr. Decision not yet agreed.`,speaker:i%2?'Me':'Participants'})));
  const compact = useCompactLayout();
  return <TooltipProvider><main tabIndex={-1} style={{height:'100dvh',display:'flex',flexDirection:'column',padding:16,gap:12}}>
    <LifecycleProbe /><ClockProbe /><h1 style={{fontSize:20,fontWeight:600}}>ClawScribe UI regression fixture</h1>
    <p>Real production components; simulated provider and recording data. Not a live meeting.</p>
    <div style={{display:'flex',gap:12,flexWrap:'wrap'}}>
      <button type="button" onClick={() => setMeetingId(id=>id==='meeting-one'?'meeting-two':'meeting-one')}>Switch meeting</button>
      <button type="button" onClick={() => setSegments(previous=>[...previous,{id:`row-${previous.length}`,timestamp:previous.length*3,text:'A new finalized transcript segment is immediately readable.',speaker:'Me'}])}>Append finalized segment</button>
      <button type="button" onClick={() => document.documentElement.classList.toggle('dark')}>Toggle theme</button>
      <span data-testid="layout-mode">{compact?'compact':'wide'}</span>
    </div>
    <div style={{width:320,maxWidth:'100%'}}>
      {!deleted && <MeetingNavigationItem title="Quarterly review" href="#opened" active={false} onOpen={()=>{}} onEdit={()=>{}} onDelete={()=>setDeleting(true)} />}
      {deleted && <p role="status">Meeting deleted</p>}
    </div>
    <section aria-label="Live transcript fixture" style={{flex:1,minHeight:0,overflow:'hidden',border:'1px solid hsl(var(--border))'}}>
      <VirtualizedTranscriptView segments={segments} isRecording enableStreaming={false} showConfidence={false} />
    </section>
    <MeetingChat meetingId={meetingId} provider="fixture-provider" model="fixture-model" />
    <ConfirmationModal isOpen={deleting} onCancel={()=>setDeleting(false)} text="Delete the selected fixture meeting? This cannot be undone."
      onConfirm={async()=>{if((window as any).failDelete)throw Error('Simulated storage failure');setDeleted(true);setDeleting(false);}} />
  </main></TooltipProvider>;
}
const fixtureRoot = createRoot(document.getElementById('root')!);
(window as any).unmountFixture = () => fixtureRoot.unmount();
fixtureRoot.render(<React.StrictMode><RecordingStateProvider><Fixture /></RecordingStateProvider></React.StrictMode>);
