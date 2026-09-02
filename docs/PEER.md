# Team backend (peer network)

Git Companion uses a shared FastAPI backend to relay push events between team
members and manage project membership.

## Device registration

On first launch the app registers the device:

```
POST /devices/register
{ "name": "my-machine" }
→ { "id": "device-uuid", "name": "my-machine", "user_id": "..." }
```

The backend URL is stored in `config.json` as `peer.backend_url`.

## Project management

A **project** is a shared namespace for push events. Devices join via a join
code generated when the owner invites a device by email.

```
POST /projects           # owner: create project
DELETE /projects/{id}    # owner: delete project
GET  /projects/{id}      # member: project info
GET  /projects/{id}/members  # member: list members
POST /projects/join      # device: join with invite code
DELETE /projects/{id}/members/{device_id}  # owner: remove member
```

## Team events

When a pre-push hook fires, the binary POSTs to the backend:

```
POST /events
{
  "project_id": "...",
  "event_kind": "main_push" | "branch_push" | "release",
  "repo_name": "my-repo",
  "payload": "{...}"   # JSON string
}
→ { "id": "event-uuid" }
```

The backend verifies the caller is a project member, persists the event, and
asynchronously fans it out to all other online members.

## Long-poll

Members poll for new events:

```
POST /events/poll
{ "project_id": "...", "device_id": "..." }
→ { "event": { "id", "project_id", "event_kind", "repo_name", "payload", "created_at" } | null }
```

If no new event arrives within 25 seconds the poll returns `null`. Clients
should immediately re-poll after each response.

## Membership by email

Owners invite teammates by email:

```
POST /projects/{id}/members/email
{ "email": "teammate@example.com", "name": "Teammate" }
→ { "device_id": null, "pending": true }
```

The backend generates a join code and sends an invite email. Once the invitee
registers and joins the project, the `pending` flag is cleared.

```
DELETE /projects/{id}/members/email/{email}  # owner: revoke invite
```

## End-to-end flow

1. Alice registers her device → `POST /devices/register`
2. Alice creates a project → `POST /projects`
3. Alice invites Bob by email → `POST /projects/{id}/members/email`
4. Bob registers his device → `POST /devices/register`
5. Bob joins the project with the invite code → `POST /projects/join`
6. Bob pushes → pre-push hook fires → `git-companion hook emit`
7. Binary POSTs to `/events` → backend fans out to Alice's poll
8. Alice's app shows the new push in the Team inbox
