-- Stalky migration 0001: public.profiles.
--
-- Optional future profile persistence, kept separate from the current identity
-- endpoint. No row is auto-created here: sign-in alone never writes data.

begin;

-- ---------------------------------------------------------------------------
-- Table
-- ---------------------------------------------------------------------------
create table public.profiles (
    id           uuid        primary key references auth.users (id) on delete cascade,
    display_name text        not null default '',
    avatar_url   text,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now(),

    constraint profiles_avatar_url_length check (length(avatar_url) <= 2048),
    constraint profiles_display_name_length check (length(display_name) <= 120)
);

-- ---------------------------------------------------------------------------
-- updated_at trigger
-- ---------------------------------------------------------------------------
create or replace function public.set_updated_at()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
    new.updated_at := now();
    return new;
end;
$$;

create trigger profiles_set_updated_at
    before update on public.profiles
    for each row
    execute function public.set_updated_at();

-- ---------------------------------------------------------------------------
-- Row level security: enable and force.
-- Force makes the table owner subject to policies; PostgreSQL superusers and
-- roles with BYPASSRLS remain outside RLS and must never back client requests.
-- ---------------------------------------------------------------------------
alter table public.profiles enable row level security;
alter table public.profiles force row level security;

-- ---------------------------------------------------------------------------
-- Least-privilege policies for the authenticated role, keyed to auth.uid().
-- All rows are private; policies authorize only the owning user.
-- There is deliberately no delete policy: users may never remove their row.
-- ---------------------------------------------------------------------------
create policy "profiles_select_own"
    on public.profiles for select
    to authenticated
    using (id = auth.uid());

create policy "profiles_insert_own"
    on public.profiles for insert
    to authenticated
    with check (id = auth.uid());

create policy "profiles_update_own"
    on public.profiles for update
    to authenticated
    using (id = auth.uid())
    with check (id = auth.uid());

-- ---------------------------------------------------------------------------
-- Revoke unsafe grants and grant only what the authenticated role needs.
-- Ordering matters: revoke from PUBLIC/anon (the broad grants) first, then
-- grant the narrow set to authenticated.
--
-- service_role is intentionally untouched: it bypasses RLS and is not enabled
-- for the client-facing backend. If a future deployment needs server-side
-- admin access it must be justified and scoped separately.
-- ---------------------------------------------------------------------------
revoke all on table public.profiles from anon;
revoke all on table public.profiles from public;
revoke execute on function public.set_updated_at() from public, anon, authenticated;
grant select, insert, update on table public.profiles to authenticated;

commit;
