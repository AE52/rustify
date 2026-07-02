import { cardCls, SectionTitle } from '../../../components/ui'

/**
 * Persistent storage tab. The C6 schema reserves `persistent_storages`, but
 * the Phase 1 REST surface (C5) exposes no storage endpoints yet, so this tab
 * is a placeholder until the API lands.
 */
export default function ApplicationStorage() {
  return (
    <div className="max-w-2xl">
      <SectionTitle>Persistent storage</SectionTitle>
      <div className={`${cardCls} border-dashed text-sm text-zinc-400`}>
        <p>
          Volume mounts (<code className="text-zinc-300">persistent_storages</code>) are provisioned in
        the database schema but not exposed by the Phase 1 API yet.
        </p>
        <p className="mt-2">
          Once the endpoints ship, you will manage named volumes and host-path mounts for this
          application here.
        </p>
      </div>
    </div>
  )
}
