import React from 'react';

type Pending = { command: string; args: Record<string, unknown>; resolve: (value: unknown) => void; reject: (error: Error) => void };
const root = window as any;
const fixture = root.chatFixture = {
  calls: [] as {command: string; args: Record<string, unknown>}[],
  pending: [] as Pending[],
  failSends: 0,
  resolve(command: string, override?: unknown) {
    const index = this.pending.findIndex(item => item.command === command);
    if (index < 0) throw Error(`No pending ${command}`);
    const item = this.pending.splice(index,1)[0];
    const fallback = command === 'api_chat_history' ? [] : {
      id: `reply-${item.args.requestId}`, meeting_id: item.args.meetingId,
      role: 'assistant', content: 'The proposal is not an agreed decision.', created_at: '2026-01-01', context_truncated: false,
    };
    item.resolve(override === undefined ? fallback : override);
  },
};
export function invoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
  fixture.calls.push({command,args});
  if (command === 'api_chat_history' && !root.holdChatHistory) return Promise.resolve([] as T);
  if (command === 'api_chat_clear') return Promise.resolve(0 as T);
  if (command === 'api_chat_send' && fixture.failSends > 0) {
    fixture.failSends--;
    return Promise.reject(Error('Simulated provider failure'));
  }
  if (!['api_chat_history','api_chat_send'].includes(command)) throw Error(`Unexpected native invocation: ${command}`);
  return new Promise<T>((resolve,reject)=>fixture.pending.push({command,args,resolve:resolve as any,reject}));
}
const listeners: Record<string, Set<() => void>> = Object.fromEntries(['started','stopped','paused','resumed'].map(name=>[name,new Set()]));
const recording = root.recordingFixture = {
  state: {is_recording:true,is_paused:false,is_active:true,recording_duration:123.2,active_duration:123.2},
  lifecycleRenders: 0,
  calls: 0,
  holdReads: false,
  pending: [] as (()=>void)[],
  listenerCount() { return Object.values(listeners).reduce((sum,set)=>sum+set.size,0); },
  resolveReads() { const pending=this.pending.splice(0); pending.forEach(resolve=>resolve()); },
  emit(event: string) {
    if(event==='paused') Object.assign(this.state,{is_paused:true,is_active:false});
    if(event==='resumed') Object.assign(this.state,{is_paused:false,is_active:true});
    if(event==='stopped') Object.assign(this.state,{is_recording:false,is_paused:false,is_active:false});
    if(event==='started') Object.assign(this.state,{is_recording:true,is_paused:false,is_active:true,recording_duration:0,active_duration:0});
    listeners[event].forEach(callback=>callback());
  },
};
const subscribe = async (name: string, callback: ()=>void) => {
  listeners[name].add(callback);
  return () => {listeners[name].delete(callback);};
};
export const recordingService = {
  getRecordingState: async () => {
    recording.calls++;
    const snapshot={...recording.state};
    if(recording.holdReads) await new Promise<void>(resolve=>recording.pending.push(resolve));
    return snapshot;
  },
  onRecordingStarted: (callback:()=>void)=>subscribe('started',callback),
  onRecordingStopped: (callback:()=>void)=>subscribe('stopped',callback),
  onRecordingPaused: (callback:()=>void)=>subscribe('paused',callback),
  onRecordingResumed: (callback:()=>void)=>subscribe('resumed',callback),
};
export default function Link({prefetch,children,...props}: any) { return <a {...props}>{children}</a>; }
