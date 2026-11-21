import { readFileSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { performance } from 'node:perf_hooks';

// Run with Node 24+. All meetings, transcripts and 384-dimensional vectors are synthetic.
const db = new DatabaseSync(':memory:');
const migrations = new URL('../frontend/src-tauri/migrations/', import.meta.url);
const migration = name => db.exec(readFileSync(new URL(`${name}.sql`, migrations), 'utf8'));
const retrieval = readFileSync(new URL('../frontend/src-tauri/src/knowledge/retrieval.rs', import.meta.url), 'utf8');
const meetings = 240;
const chunksPerMeeting = 600;
const pageSize = 512;
const space = 'synthetic-384';

const tokens = 'Wir prüfen am Dienstag die offenen Punkte und planen die nächsten Schritte gemeinsam'.split(' ');
const reviewRows = Array.from({ length: 1200 }, (_, i) => {
  const start = i * 6;
  return {
    id: `transcript-${i}`, text: `${tokens.join(' ')}.`, audio_start_time: start, audio_end_time: start + 5.5,
    speaker: i % 3 === 0 ? 'Participants' : 'Me',
    timestamp: '00:00:00', duration: 5.5,
    word_timestamps: tokens.map((text, index) => ({ text, start: start + index * .4, end: start + index * .4 + .3 })),
  };
});
const timeline = reviewRows.map(({ id, text, audio_start_time, audio_end_time, speaker }) =>
  ({ id, text, audio_start_time, audio_end_time, speaker }));
const wireSize = value => (Buffer.byteLength(JSON.stringify(value)) / 1024).toFixed(1);
console.log(`Two-hour transcript, ${reviewRows.length} passages: ${wireSize(reviewRows)} KiB with word timing, ${wireSize(timeline)} KiB for the timeline, ${wireSize(reviewRows.slice(0, 100))} KiB for the first text page.`);

db.exec(`CREATE TABLE meetings(id TEXT PRIMARY KEY, title TEXT, created_at TEXT);
CREATE TABLE transcripts(id TEXT PRIMARY KEY, meeting_id TEXT, transcript TEXT, speaker TEXT,
 timestamp TEXT, audio_start_time REAL, audio_end_time REAL, duration REAL, word_timestamps_json TEXT);`);
migration('20261007000000_knowledge_index');
migration('20261007000001_knowledge_conversations');
migration('20261008000001_knowledge_documents');
migration('20261121000010_knowledge_lookup');

const source = db.prepare('INSERT INTO knowledge_sources(id,kind,meeting_id,semantic_revision,semantic_space) VALUES (?, ?, ?,1,?)');
const meeting = db.prepare('INSERT INTO meetings VALUES (?, ?, ?)');
const publish = db.prepare('UPDATE knowledge_sources SET semantic_revision=1, semantic_space=? WHERE meeting_id=?');
const chunk = db.prepare('INSERT INTO knowledge_chunks VALUES (?, ?, 1, 1, ?, ?, 0, 41, ?, ?)');
const vector = db.prepare('INSERT INTO knowledge_vectors VALUES (?, ?, 384, zeroblob(1536))');
const addChunks = (sourceId, count, serial) => {
  for (let ordinal = 0; ordinal < count; ordinal++) {
    // Hash-like IDs interleave sources in the global keyset, like the production fingerprints.
    const id = Math.imul(serial * chunksPerMeeting + ordinal + 1, 0x9e3779b1).toString(16);
    const acknowledgements = ['Ja.', 'Genau.', 'Richtig.', 'Danke.'];
    const body = ordinal % 6 === 0 && sourceId.startsWith('meeting:')
      ? acknowledgements[Math.floor(ordinal / 6) % acknowledgements.length]
      : `Fixture planning discussion, review and decision for item ${serial}-${ordinal}.`;
    chunk.run(id, sourceId, ordinal, `seg-${ordinal}`, id, body);
    vector.run(id, space);
  }
};
db.exec('BEGIN');
for (let i = 0; i < meetings; i++) {
  meeting.run(`m${i}`, `Sample meeting ${i}`, '2026-06-01');
  publish.run(space, `m${i}`);
  addChunks(`meeting:m${i}`, chunksPerMeeting, i);
}
for (let i = 0; i < 3; i++) {
  source.run(`document:d${i}`, 'document', null, space);
  db.prepare('INSERT INTO knowledge_documents(id,source_id,display_name,format,file_size,sha256,storage_name) VALUES (?, ?, ?, ?, 10, ?, ?)')
    .run(`d${i}`, `document:d${i}`, 'Sample notes', 'txt', String(i).repeat(64), `d${i}.txt`);
  db.prepare('INSERT INTO knowledge_document_attachments (meeting_id,document_id) VALUES (?, ?)').run(`m${i}`, `d${i}`);
  addChunks(`document:d${i}`, 20, meetings + i);
}
db.exec('COMMIT; ANALYZE;');

const rustString = name => retrieval.match(new RegExp(`const ${name}: &str = "([^"\n]+)";`))[1];
const baseline = `${rustString('SCOPE_CTE')} SELECT c.id,v.vector,s.id
  FROM knowledge_vectors v JOIN knowledge_chunks c ON c.id=v.chunk_id JOIN knowledge_sources s ON s.id=c.source_id
  WHERE ${rustString('SCOPED_SOURCE')} AND v.space=? AND s.semantic_space=? AND s.semantic_revision=s.revision
  AND c.revision=s.revision AND c.generation=s.generation AND c.id>? ORDER BY c.id LIMIT ?`;
const bySource = rustString('SEMANTIC_SOURCE_PAGE');

const median = samples => samples.sort((a, b) => a - b)[Math.floor(samples.length / 2)].toFixed(2);
function measure(label, sql, scopeParams) {
  const page = db.prepare(sql);
  page.setReturnArrays(true);
  const plan = db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all(...scopeParams()[0], '', pageSize).map(row => row.detail);
  const samples = [];
  let count = 0;
  for (let run = 0; run < 4; run++) {
    count = 0;
    const start = performance.now();
    for (const params of scopeParams()) {
      let after = '';
      while (true) {
        const rows = page.all(...params, after, pageSize);
        count += rows.length;
        if (!rows.length) break;
        after = rows.at(-1)[0];
      }
    }
    if (run) samples.push(performance.now() - start);
  }
  console.log(`${label}: ${count} vectors, median ${median(samples)} ms\n  ${plan.join('\n  ')}`);
  return count;
}

console.log(`SQLite ${db.prepare('SELECT sqlite_version() version').get().version}, ${meetings} meetings, ${meetings * chunksPerMeeting + 60} vectors`);
const sizes = [1, 5, 60, meetings];
const scopeParams = size => {
  const allowed = JSON.stringify(Array.from({ length: size }, (_, i) => `m${i}`));
  const documents = JSON.stringify(['d0', 'd1']);
  return [allowed, documents];
};
const expected = size => size * chunksPerMeeting + Math.min(size, 2) * 20;
for (const size of sizes) {
  const count = measure(`Global query / ${size} meetings`, baseline, () => [[...scopeParams(size), space, space]]);
  if (count !== expected(size)) throw new Error(`Incorrect baseline result count for ${size} meetings`);
}
migration('20261121125000_knowledge_vector_source_page');
migration('20261121125001_knowledge_shared_vectors');
db.exec('ANALYZE');
const sourceList = db.prepare(`${rustString('SCOPE_CTE')} SELECT s.id FROM knowledge_sources s WHERE ${rustString('SCOPED_SOURCE')} AND ${rustString('PUBLISHED_SOURCE')}`);
const selectedParams = size => sourceList.all(...scopeParams(size), space).map(({ id }) => [id, space]);
const directCount = () => db.prepare('SELECT count(*) AS count FROM knowledge_vectors').get().count;
const previousVectors = directCount();
// One sample tests old-format rows directly after migration, before rebuilding any sources.
const migrated = measure('Source query / migrated sample', bySource, () => selectedParams(5));
if (migrated !== expected(5) || directCount() !== previousVectors) throw new Error('Migration changed old vectors');

// In this fixture repeated samples recur before the production 64-entry cache evicts them.
db.exec(`BEGIN;
INSERT INTO knowledge_vector_aliases(chunk_id,vector_chunk_id)
SELECT id,owner FROM (
  SELECT id, FIRST_VALUE(id) OVER (PARTITION BY source_id,text ORDER BY ordinal) owner
  FROM knowledge_chunks WHERE length(CAST(text AS BLOB)) <= 96 AND source_id LIKE 'meeting:%'
) WHERE id<>owner;
DELETE FROM knowledge_vectors WHERE chunk_id IN (SELECT chunk_id FROM knowledge_vector_aliases);
COMMIT; ANALYZE;`);
const packedVectors = directCount();
const saved = previousVectors - packedVectors;
console.log(`Repeated short rows: ${previousVectors} → ${packedVectors} stored vectors; ${(100 * saved / previousVectors).toFixed(1)}% fewer inputs and ${(saved * 1536 / 1024 ** 2).toFixed(1)} MiB of vector payload avoided (before alias overhead).`);
if (saved !== meetings * (chunksPerMeeting / 6 - 4)) throw new Error('Unexpected alias count');
for (const size of sizes) {
  const scopedCount = measure(`Source query / ${size} meetings`, bySource, () => selectedParams(size));
  if (scopedCount !== expected(size)) throw new Error(`Incorrect scoped result count for ${size} meetings`);
}
