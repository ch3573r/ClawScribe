/** Native-owned ordering; decimal strings preserve the complete u64 range. */
export type RecordingIdentity = { id: string; generation: string };
const maximumGeneration = '18446744073709551615';
export function recordingIdentity(id: unknown, generation: unknown): RecordingIdentity | null {
  if (typeof id !== 'string' || !id || typeof generation !== 'string' ||
    !/^[1-9][0-9]*$/.test(generation) || generation.length > maximumGeneration.length ||
    (generation.length === maximumGeneration.length && generation > maximumGeneration)) return null;
  return { id, generation };
}
export function compareGeneration(left: string, right: string): number {
  return left.length === right.length ? (left === right ? 0 : left > right ? 1 : -1) : left.length - right.length;
}
export function sameIdentity(left: RecordingIdentity | null, right: RecordingIdentity): boolean {
  return left !== null && left.id === right.id && left.generation === right.generation;
}
