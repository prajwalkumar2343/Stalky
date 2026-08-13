-- Stalky migration 0002: tenant-scoped application state.
--
-- The caller identity is Supabase auth.users.id. Every application row carries
-- that UUID and is protected by the same-owner RLS policy. The Rust backend
-- uses the authenticated Supabase JWT for reads and writes; service-role
-- connections are intentionally not accepted by the public client boundary.

begin;

create table public.memories (
    id          uuid primary key,
    user_id     uuid not null references auth.users (id) on delete cascade,
    title       text not null,
    content     text not null,
    created_at  timestamptz not null default now(),

    constraint memories_title_length check (char_length(title) between 1 and 240),
    constraint memories_content_length check (char_length(content) between 1 and 8000)
);

create table public.todos (
    id          uuid primary key,
    user_id     uuid not null references auth.users (id) on delete cascade,
    title       text not null,
    done        boolean not null default false,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now(),

    constraint todos_title_length check (char_length(title) between 1 and 500)
);

create trigger todos_set_updated_at
    before update on public.todos
    for each row
    execute function public.set_updated_at();

create table public.mini_app_records (
    id          uuid primary key,
    user_id     uuid not null references auth.users (id) on delete cascade,
    mini_app_id text not null,
    record_type text not null default 'record',
    "values"    jsonb not null default '{}'::jsonb,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now(),

    constraint mini_app_records_app_id_length check (char_length(mini_app_id) between 1 and 120),
    constraint mini_app_records_type_length check (char_length(record_type) between 1 and 80),
    constraint mini_app_records_values_object check (jsonb_typeof("values") = 'object')
);

create trigger mini_app_records_set_updated_at
    before update on public.mini_app_records
    for each row
    execute function public.set_updated_at();

create table public.agent_runs (
    id               uuid primary key,
    user_id          uuid not null references auth.users (id) on delete cascade,
    session_id       uuid not null,
    state            text not null,
    phase            text not null,
    request_payload  jsonb not null default '{}'::jsonb,
    reply            text,
    emotion          text not null default 'neutral',
    created_emotion  text,
    actions          jsonb not null default '[]'::jsonb,
    children         jsonb not null default '[]'::jsonb,
    error            text,
    created_at       timestamptz not null default now(),
    updated_at       timestamptz not null default now(),

    constraint agent_runs_state check (state in ('queued', 'running', 'completed', 'failed', 'interrupted', 'cancelled')),
    constraint agent_runs_phase check (phase in ('admitted', 'planning', 'delegating', 'synthesizing', 'completed', 'failed', 'interrupted', 'cancelled')),
    constraint agent_runs_actions_array check (jsonb_typeof(actions) = 'array'),
    constraint agent_runs_children_array check (jsonb_typeof(children) = 'array')
);

create trigger agent_runs_set_updated_at
    before update on public.agent_runs
    for each row
    execute function public.set_updated_at();

create table public.agent_run_events (
    run_id      uuid not null references public.agent_runs (id) on delete cascade,
    sequence    bigint not null,
    event_type  text not null,
    payload     jsonb not null default '{}'::jsonb,
    created_at  timestamptz not null default now(),

    primary key (run_id, sequence)
);

create table public.idempotency_keys (
    user_id      uuid not null references auth.users (id) on delete cascade,
    key          text not null,
    request_hash text not null,
    run_id       uuid not null references public.agent_runs (id) on delete cascade,
    created_at   timestamptz not null default now(),

    primary key (user_id, key),
    constraint idempotency_key_length check (char_length(key) between 1 and 200)
);

create table public.devices (
    id           uuid primary key,
    user_id      uuid not null references auth.users (id) on delete cascade,
    platform     text not null,
    device_name  text not null default '',
    metadata     jsonb not null default '{}'::jsonb,
    last_seen_at timestamptz not null default now(),
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now(),

    constraint devices_platform_length check (char_length(platform) between 1 and 40),
    constraint devices_name_length check (char_length(device_name) <= 120),
    constraint devices_metadata_object check (jsonb_typeof(metadata) = 'object')
);

create trigger devices_set_updated_at
    before update on public.devices
    for each row
    execute function public.set_updated_at();

create table public.uploads (
    id           uuid primary key,
    user_id      uuid not null references auth.users (id) on delete cascade,
    object_key   text not null,
    mime_type    text not null,
    byte_size    bigint not null,
    status       text not null default 'pending',
    metadata     jsonb not null default '{}'::jsonb,
    created_at   timestamptz not null default now(),
    completed_at timestamptz,

    constraint uploads_object_key_length check (char_length(object_key) between 1 and 1024),
    constraint uploads_mime_type_length check (char_length(mime_type) between 1 and 120),
    constraint uploads_byte_size_positive check (byte_size > 0),
    constraint uploads_status check (status in ('pending', 'ready', 'failed', 'deleted')),
    constraint uploads_metadata_object check (jsonb_typeof(metadata) = 'object')
);

create index memories_user_created_idx on public.memories (user_id, created_at desc);
create index todos_user_created_idx on public.todos (user_id, created_at desc);
create index mini_app_records_user_app_type_idx on public.mini_app_records (user_id, mini_app_id, record_type, created_at desc);
create index agent_runs_user_created_idx on public.agent_runs (user_id, created_at desc);
create index agent_runs_queue_idx on public.agent_runs (state, created_at);
create index agent_run_events_run_sequence_idx on public.agent_run_events (run_id, sequence);
create index devices_user_seen_idx on public.devices (user_id, last_seen_at desc);
create index uploads_user_created_idx on public.uploads (user_id, created_at desc);

alter table public.memories enable row level security;
alter table public.memories force row level security;
alter table public.todos enable row level security;
alter table public.todos force row level security;
alter table public.mini_app_records enable row level security;
alter table public.mini_app_records force row level security;
alter table public.agent_runs enable row level security;
alter table public.agent_runs force row level security;
alter table public.agent_run_events enable row level security;
alter table public.agent_run_events force row level security;
alter table public.idempotency_keys enable row level security;
alter table public.idempotency_keys force row level security;
alter table public.devices enable row level security;
alter table public.devices force row level security;
alter table public.uploads enable row level security;
alter table public.uploads force row level security;

create policy memories_owner on public.memories for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy todos_owner on public.todos for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy mini_app_records_owner on public.mini_app_records for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy agent_runs_owner on public.agent_runs for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy agent_run_events_owner on public.agent_run_events for all to authenticated
    using (exists (select 1 from public.agent_runs where id = run_id and user_id = auth.uid()))
    with check (exists (select 1 from public.agent_runs where id = run_id and user_id = auth.uid()));
create policy idempotency_keys_owner on public.idempotency_keys for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy devices_owner on public.devices for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy uploads_owner on public.uploads for all to authenticated
    using (user_id = auth.uid()) with check (user_id = auth.uid());

revoke all on table public.memories, public.todos, public.mini_app_records,
    public.agent_runs, public.agent_run_events, public.idempotency_keys,
    public.devices, public.uploads from anon, public;
grant select, insert, update, delete on table public.memories, public.todos,
    public.mini_app_records, public.agent_runs, public.agent_run_events,
    public.idempotency_keys, public.devices, public.uploads to authenticated;

commit;

