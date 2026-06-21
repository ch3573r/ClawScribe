# Planner Export

ClawScribe exports reviewed action items to Microsoft Planner through Microsoft
Graph delegated auth. Planner export is intentionally review-first because AI
summaries can misread ownership, due dates, or action boundaries.

## Current UX

1. User signs in to Microsoft.
2. User opens Planner export from a meeting.
3. ClawScribe parses candidate action items from the current notes.
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

Planner task creation is not naturally idempotent. ClawScribe keeps local
metadata for exported action items so retrying a meeting does not create the
same task again.

The ledger may store:

- local action IDs
- task IDs
- plan/bucket IDs
- dedupe keys
- status and sanitized error codes
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
