create table if not exists public.memories (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade default auth.uid(),
  created_at timestamptz not null default now(),
  transcript text not null default '',
  reply text not null,
  image_path text not null,
  model text not null
);

create index if not exists memories_user_created_at_idx
  on public.memories (user_id, created_at desc);

alter table public.memories enable row level security;

drop policy if exists "Users can read their memories" on public.memories;
create policy "Users can read their memories"
  on public.memories for select
  to authenticated
  using ((select auth.uid()) = user_id);

drop policy if exists "Users can create their memories" on public.memories;
create policy "Users can create their memories"
  on public.memories for insert
  to authenticated
  with check ((select auth.uid()) = user_id);

drop policy if exists "Users can delete their memories" on public.memories;
create policy "Users can delete their memories"
  on public.memories for delete
  to authenticated
  using ((select auth.uid()) = user_id);

insert into storage.buckets (id, name, public, file_size_limit, allowed_mime_types)
values ('diary-pages', 'diary-pages', false, 6291456, array['image/png'])
on conflict (id) do update set
  public = excluded.public,
  file_size_limit = excluded.file_size_limit,
  allowed_mime_types = excluded.allowed_mime_types;

drop policy if exists "Users can read their diary pages" on storage.objects;
create policy "Users can read their diary pages"
  on storage.objects for select
  to authenticated
  using (
    bucket_id = 'diary-pages'
    and (storage.foldername(name))[1] = (select auth.uid())::text
  );

drop policy if exists "Users can create their diary pages" on storage.objects;
create policy "Users can create their diary pages"
  on storage.objects for insert
  to authenticated
  with check (
    bucket_id = 'diary-pages'
    and (storage.foldername(name))[1] = (select auth.uid())::text
  );

drop policy if exists "Users can delete their diary pages" on storage.objects;
create policy "Users can delete their diary pages"
  on storage.objects for delete
  to authenticated
  using (
    bucket_id = 'diary-pages'
    and (storage.foldername(name))[1] = (select auth.uid())::text
  );
