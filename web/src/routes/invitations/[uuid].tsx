import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from 'react-router'
import { api, type InvitationInfo, type Team } from '../../api/client'
import { roleLabel } from '../../lib/roles'
import { btnGhost, btnPrimary, cardCls, ErrorNote, PageTitle } from '../../components/ui'

/**
 * Invitation landing page: `GET /invitations/{uuid}` renders who invited whom;
 * accepting `POST`s the same path, which joins the team and switches the active
 * team server-side. We then invalidate all queries and land on the dashboard.
 */
export default function InvitationPage() {
  const { uuid = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const info = useQuery({
    queryKey: ['invitation', uuid],
    queryFn: () => api.get<InvitationInfo>(`/invitations/${uuid}`),
    retry: false,
  })

  const accept = useMutation({
    mutationFn: () => api.post<Team>(`/invitations/${uuid}`),
    onSuccess: () => {
      queryClient.invalidateQueries()
      navigate('/')
    },
  })

  if (info.isPending) return <p className="text-sm text-zinc-500">Loading…</p>
  if (info.isError) {
    return (
      <div className="mx-auto max-w-md">
        <PageTitle>Invitation</PageTitle>
        <div className="mt-4">
          <ErrorNote error={info.error} />
        </div>
      </div>
    )
  }

  const data = info.data

  return (
    <div className="mx-auto flex max-w-md flex-col gap-4">
      <PageTitle>Team invitation</PageTitle>
      <div className={`${cardCls} flex flex-col gap-3`}>
        <p className="text-sm text-zinc-300">
          You have been invited to join{' '}
          <span className="font-semibold text-zinc-100">{data.team_name}</span> as a{' '}
          <span className="font-semibold text-zinc-100">{roleLabel(data.role)}</span>.
        </p>
        <p className="text-xs text-zinc-500">Invited: {data.email}</p>

        {data.already_member ? (
          <p className="text-sm text-amber-300">You are already a member of this team.</p>
        ) : !data.valid ? (
          <p className="text-sm text-red-400">This invitation has expired.</p>
        ) : (
          <>
            <ErrorNote error={accept.error} />
            <div className="flex gap-2">
              <button
                type="button"
                className={btnPrimary}
                disabled={accept.isPending}
                onClick={() => accept.mutate()}
              >
                {accept.isPending ? 'Joining…' : 'Accept invitation'}
              </button>
              <button type="button" className={btnGhost} onClick={() => navigate('/')}>
                Decline
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
