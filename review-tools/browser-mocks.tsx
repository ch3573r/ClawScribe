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
export const RecordingStatus = { STARTING:'starting', RECORDING:'recording' };
export function useRecordingState() { return {activeDuration:123,status:RecordingStatus.RECORDING}; }
export default function Link({prefetch,children,...props}: any) { return <a {...props}>{children}</a>; }
