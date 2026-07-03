import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  api,
  type Role,
  type Team,
  type TeamInvitation,
  type TeamMember,
} from '../api/client'
import { isAdmin, ROLE_OPTIONS, roleLabel } from '../lib/roles'
import {
  btnDanger,
  btnGhost,
  btnPrimary,
  cardCls,
  ErrorNote,
  Field,
  inputCls,
  PageTitle,
  SectionTitle,
  selectCls,
} from '../components/ui'

// ----- team details -------------------------------------------------------

function DetailsCard({ team, admin }: { team: Team; admin: boolean }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState(team.name)
  const [description, setDescription] = useState(team.description ?? '')

  const save = useMutation({
    mutationFn: () =>
      api.patch<Team>(`/teams/${team.id}`, { name, description: description || null }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['team', 'current'] })
      queryClient.invalidateQueries({ queryKey: ['teams'] })
    },
  })

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        save.mutate()
      }}
      className={`${cardCls} flex max-w-2xl flex-col gap-4`}
    >
      <SectionTitle>Team</SectionTitle>
      <Field label="Name">
        <input
          className={inputCls}
          value={name}
          disabled={!admin}
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field label="Description">
        <input
          className={inputCls}
          value={description}
          disabled={!admin}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
      {admin && (
        <>
          <ErrorNote error={save.error} />
          <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
            {save.isPending ? 'Saving…' : 'Save'}
          </button>
        </>
      )}
    </form>
  )
}

// ----- members ------------------------------------------------------------

function MemberRow({ team, member, admin }: { team: Team; member: TeamMember; admin: boolean }) {
  const queryClient = useQueryClient()
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['team', 'current', 'members'] })

  const setRole = useMutation({
    mutationFn: (role: string) =>
      api.patch<TeamMember>(`/teams/${team.id}/members/${member.uuid}`, { role }),
    onSuccess: invalidate,
  })
  const remove = useMutation({
    mutationFn: () => api.delete(`/teams/${team.id}/members/${member.uuid}`),
    onSuccess: invalidate,
  })

  return (
    <div className="flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm text-zinc-100">{member.name || member.email}</div>
        <div className="truncate text-xs text-zinc-500">{member.email}</div>
      </div>
      {admin ? (
        <select
          aria-label={`Role for ${member.email}`}
          className={`${selectCls} w-32`}
          value={member.role}
          disabled={setRole.isPending}
          onChange={(e) => setRole.mutate(e.target.value)}
        >
          {ROLE_OPTIONS.map((r) => (
            <option key={r} value={r}>
              {roleLabel(r)}
            </option>
          ))}
        </select>
      ) : (
        <span className="text-xs text-zinc-400">{roleLabel(member.role)}</span>
      )}
      {admin && (
        <button
          type="button"
          className={btnDanger}
          disabled={remove.isPending}
          onClick={() => remove.mutate()}
        >
          Remove
        </button>
      )}
      <ErrorNote error={setRole.error ?? remove.error} />
    </div>
  )
}

function MembersCard({ team, admin }: { team: Team; admin: boolean }) {
  const members = useQuery({
    queryKey: ['team', 'current', 'members'],
    queryFn: () => api.get<TeamMember[]>('/teams/current/members'),
  })

  return (
    <section className="flex max-w-2xl flex-col gap-3">
      <SectionTitle>Members</SectionTitle>
      {members.isPending && <p className="text-sm text-zinc-500">Loading…</p>}
      {members.isError && <ErrorNote error={members.error} />}
      {members.data?.map((m) => (
        <MemberRow key={m.uuid} team={team} member={m} admin={admin} />
      ))}
    </section>
  )
}

// ----- invitations --------------------------------------------------------

function InvitationsCard({ team }: { team: Team }) {
  const queryClient = useQueryClient()
  const [email, setEmail] = useState('')
  const [role, setRole] = useState<Role>('member')
  const [via, setVia] = useState<'link' | 'email'>('link')
  const [lastLink, setLastLink] = useState<string | null>(null)

  const invitations = useQuery({
    queryKey: ['team', team.id, 'invitations'],
    queryFn: () => api.get<TeamInvitation[]>(`/teams/${team.id}/invitations`),
  })
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ['team', team.id, 'invitations'] })

  const create = useMutation({
    mutationFn: () =>
      api.post<TeamInvitation>(`/teams/${team.id}/invitations`, { email, role, via }),
    onSuccess: (inv) => {
      setEmail('')
      setLastLink(inv.link ?? null)
      invalidate()
    },
  })
  const remove = useMutation({
    mutationFn: (uuid: string) => api.delete(`/invitations/${uuid}`),
    onSuccess: invalidate,
  })

  return (
    <section className="flex max-w-2xl flex-col gap-3">
      <SectionTitle>Invitations</SectionTitle>
      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          create.mutate()
        }}
        className={`${cardCls} flex flex-col gap-3`}
      >
        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-48 flex-1">
            <Field label="Email">
              <input
                type="email"
                className={inputCls}
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="teammate@example.com"
              />
            </Field>
          </div>
          <Field label="Role">
            <select
              aria-label="Invitation role"
              className={`${selectCls} w-28`}
              value={role}
              onChange={(e) => setRole(e.target.value as Role)}
            >
              {ROLE_OPTIONS.map((r) => (
                <option key={r} value={r}>
                  {roleLabel(r)}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Via">
            <select
              aria-label="Invitation via"
              className={`${selectCls} w-24`}
              value={via}
              onChange={(e) => setVia(e.target.value as 'link' | 'email')}
            >
              <option value="link">Link</option>
              <option value="email">Email</option>
            </select>
          </Field>
          <button type="submit" className={btnPrimary} disabled={create.isPending || !email}>
            {create.isPending ? 'Inviting…' : 'Invite'}
          </button>
        </div>
        <ErrorNote error={create.error} />
        {lastLink && (
          <div className="rounded-md border border-emerald-900 bg-emerald-950/40 px-3 py-2 text-xs">
            <span className="text-emerald-300">Invitation link: </span>
            <code data-testid="invite-link" className="font-mono text-emerald-200">
              {lastLink}
            </code>
          </div>
        )}
      </form>

      {invitations.data?.map((inv) => (
        <div
          key={inv.uuid}
          className="flex items-center gap-3 rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-2.5"
        >
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm text-zinc-200">{inv.email}</div>
            <div className="text-xs text-zinc-500">
              {roleLabel(inv.role)} · via {inv.via}
            </div>
          </div>
          {inv.link && <code className="truncate font-mono text-xs text-sky-400">{inv.link}</code>}
          <button
            type="button"
            className={btnGhost}
            disabled={remove.isPending}
            onClick={() => remove.mutate(inv.uuid)}
          >
            Revoke
          </button>
        </div>
      ))}
    </section>
  )
}

// ----- create team --------------------------------------------------------

function CreateTeamCard() {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')

  const create = useMutation({
    mutationFn: () => api.post<Team>('/teams', { name }),
    onSuccess: () => {
      setName('')
      queryClient.invalidateQueries({ queryKey: ['teams'] })
    },
  })

  return (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        create.mutate()
      }}
      className={`${cardCls} flex max-w-2xl items-end gap-3`}
    >
      <div className="flex-1">
        <Field label="New team name">
          <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
      </div>
      <button type="submit" className={btnPrimary} disabled={create.isPending || !name}>
        {create.isPending ? 'Creating…' : 'Create team'}
      </button>
      <ErrorNote error={create.error} />
    </form>
  )
}

// ----- page ---------------------------------------------------------------

export default function TeamsPage() {
  const current = useQuery({
    queryKey: ['team', 'current'],
    queryFn: () => api.get<Team>('/teams/current'),
  })

  if (current.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (current.isError) return <ErrorNote error={current.error} />

  const team = current.data
  const admin = isAdmin(team.role)

  return (
    <div className="flex flex-col gap-8">
      <PageTitle>Team settings</PageTitle>
      <DetailsCard team={team} admin={admin} />
      <MembersCard team={team} admin={admin} />
      {admin && <InvitationsCard team={team} />}
      <div className="flex flex-col gap-3">
        <SectionTitle>Create a new team</SectionTitle>
        <CreateTeamCard />
      </div>
    </div>
  )
}
