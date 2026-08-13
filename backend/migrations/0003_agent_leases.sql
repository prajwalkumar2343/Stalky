-- Stalky migration 0003: durable agent worker leases and server-only credentials.

begin;

alter table public.agent_runs
    add column attempt integer not null default 0,
    add column max_attempts integer not null default 3,
    add column next_attempt_at timestamptz not null default now(),
    add column lease_owner text,
    add column lease_token uuid,
    add column lease_expires_at timestamptz,
    add column credential_ciphertext text;

alter table public.agent_runs
    add constraint agent_runs_attempts_valid check (attempt >= 0 and max_attempts between 1 and 10),
    add constraint agent_runs_lease_fields_consistent check (
        (lease_owner is null and lease_token is null and lease_expires_at is null)
        or (lease_owner is not null and lease_token is not null and lease_expires_at is not null)
    );

create index agent_runs_claim_idx
    on public.agent_runs (state, next_attempt_at, created_at)
    where state in ('queued', 'running');

create index agent_runs_lease_expiry_idx
    on public.agent_runs (lease_expires_at)
    where state = 'running';

commit;
