import { useState, type FormEvent } from 'react'
import { useMutation } from '@tanstack/react-query'
import { api, type Application } from '../../../api/client'
import { useApplication } from './index'
import { btnPrimary, cardCls, ErrorNote, Field, inputCls, SectionTitle } from '../../../components/ui'

export default function ApplicationDomains() {
  const { app, refetch } = useApplication()
  const [fqdn, setFqdn] = useState(app.fqdn ?? '')

  const save = useMutation({
    mutationFn: () => api.patch<Application>(`/applications/${app.uuid}`, { fqdn: fqdn || null }),
    onSuccess: () => refetch(),
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    save.mutate()
  }

  const domains = fqdn
    .split(',')
    .map((d) => d.trim())
    .filter(Boolean)

  return (
    <form onSubmit={submit} className={`${cardCls} flex max-w-2xl flex-col gap-4`}>
      <SectionTitle>Domains</SectionTitle>
      <Field label="Fully qualified domain name(s), comma separated">
        <input
          className={`${inputCls} font-mono`}
          value={fqdn}
          onChange={(e) => setFqdn(e.target.value)}
          placeholder="https://app.example.com, https://www.example.com"
        />
      </Field>
      {domains.length > 0 && (
        <ul className="flex flex-wrap gap-2">
          {domains.map((d) => (
            <li key={d} className="rounded-full border border-zinc-700 px-2.5 py-0.5 font-mono text-xs text-zinc-300">
              {d}
            </li>
          ))}
        </ul>
      )}
      <p className="text-xs text-zinc-500">
        The proxy (traefik) routes these hosts to the container port {app.ports_exposes}. TLS
        certificates are provisioned via Let&apos;s Encrypt.
      </p>
      <ErrorNote error={save.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </form>
  )
}
