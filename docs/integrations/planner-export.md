# Planner Export

ClawScribe exports reviewed action items to Microsoft Planner through Microsoft
Graph delegated auth. Planner export is intentionally review-first because AI
summaries can misread ownership, due dates, or action boundaries.

## Current UX

1. User signs in to Microsoft.
2. User opens Planner export from a meeting.
3. ClawScribe uses open reviewed meeting actions when available. Before an action
   list has been saved, it can suggest actions from summary lists or tables.
   If a regenerated summary contains other suggestions, **Review suggestions**
   opens the saved list. Choose **Add summary suggestions**, review the actions,
   and save before reopening the export.
   Suggestions available only in structured summary output go through this
   review. Completed summary checklist items stay out of export.
4. User reviews, edits, and deselects tasks.
5. User selects a plan and bucket, or creates a bucket when available.
6. ClawScribe creates only the selected tasks.
7. Local export metadata records task IDs and prevents duplicate creation.

## Graph Shape

Task creation uses the delegated `Tasks.ReadWrite` scope.

Representative call:

```http
POST https://graph.microsoft.com/v1.0/planner/tasks
Content-Type: application/json
Authorization: Bearer <access-token>
```

Planner task creation requires a plan ID. ClawScribe also requires a bucket ID
so meeting action items land in a predictable column.

## Task Mapping

Action item to task:

```json
{
  "planId": "planner-plan-id",
  "bucketId": "planner-bucket-id",
  "title": "Send revised proposal to Contoso",
  "assignments": {}
}
```

Mapping rules:

- Title is short, single-line, and user-editable.
- Longer meeting context goes into task details when supported by the export
  flow.
- No automatic assignment from transcript speaker names unless the user has
  reviewed a concrete Microsoft user mapping.
- No automatic due date unless extracted and reviewed.
- Selected bucket can differ per task when the UI supports it.

## Duplicate Protection

Planner task creation is not naturally idempotent. ClawScribe saves an attempt
before each remote create and its resulting task ID immediately afterwards.
If the description fails, re-export the same action to repair it using that
task ID. An existing description is preserved. This also applies to older
history whose task ID is available but whose details status was never saved.
Exports stop when this history cannot be read or saved.
If a batch stops, the result reports how many selected tasks were left
unattempted, including those in later buckets.

If submission was interrupted, check Planner and use **Resolve outcome** in
Export history before retrying. A saved action retains its
identity when edited and is scoped to the Microsoft account, plan, and bucket.
Adding a task to the selection preserves the history for tasks already submitted.
Ambiguous matches with older suggestions are held for review. See
[export history and recovery](microsoft-graph.md#export-history-and-recovery).

The ledger stores:

- local action IDs
- task IDs
- task titles used to identify entries in history
- plan/bucket IDs
- dedupe keys
- status and sanitized error codes
- whether the description still needs to be saved
- timestamps

It must not store Microsoft tokens or raw auth material.

## Error Handling

- `401`: token expired or invalid; prompt reconnect.
- `403`: missing consent, tenant policy, access denied, or service limit.
- `404`: plan, bucket, or user no longer exists or is not visible.
- `409` / `412`: conflict or etag mismatch on follow-up detail updates.
- `429`: respect `Retry-After` and retry only tasks that were not created.

Partial success is valid. Do not retry the full batch after some tasks have
already been created.

## Review Rules

- Keep export user-initiated.
- Keep preview/edit/deselect before creation.
- Do not broaden Graph scopes just to make discovery easier.
- Do not auto-map attendees or transcript speakers to Microsoft users without
  confirmation.
- Keep Microsoft auth independent from summary-provider auth.
