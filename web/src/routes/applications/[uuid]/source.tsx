import { useState, type FormEvent } from 'react'
import { useMutation } from '@tanstack/react-query'
import { api, type Application } from '../../../api/client'
import { useApplication } from './index'
import { btnPrimary, cardCls, ErrorNote, Field, inputCls, SectionTitle } from '../../../components/ui'

export default function ApplicationSource() {
  const { app, refetch } = useApplication()
  const [repo, setRepo] = useState(app.git_repository)
  const [branch, setBranch] = useState(app.git_branch)
  const [commitSha, setCommitSha] = useState(app.git_commit_sha)
  const [baseDirectory, setBaseDirectory] = useState(app.base_directory)
  const [publishDirectory, setPublishDirectory] = useState(app.publish_directory ?? '')
  const [installCommand, setInstallCommand] = useState(app.install_command ?? '')
  const [buildCommand, setBuildCommand] = useState(app.build_command ?? '')
  const [startCommand, setStartCommand] = useState(app.start_command ?? '')
  const [dockerfileLocation, setDockerfileLocation] = useState(app.dockerfile_location ?? '/Dockerfile')
  const [composeLocation, setComposeLocation] = useState(app.docker_compose_location ?? '/docker-compose.yaml')
  const [staticImage, setStaticImage] = useState(app.static_image ?? 'nginx:alpine')

  const save = useMutation({
    mutationFn: () =>
      api.patch<Application>(`/applications/${app.uuid}`, {
        git_repository: repo,
        git_branch: branch,
        git_commit_sha: commitSha,
        base_directory: baseDirectory,
        publish_directory: publishDirectory || null,
        install_command: installCommand || null,
        build_command: buildCommand || null,
        start_command: startCommand || null,
        dockerfile_location: dockerfileLocation,
        docker_compose_location: composeLocation,
        static_image: staticImage,
      }),
    onSuccess: () => refetch(),
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    save.mutate()
  }

  return (
    <form onSubmit={submit} className="flex max-w-2xl flex-col gap-8">
      <div className={`${cardCls} flex flex-col gap-4`}>
        <SectionTitle>Git source</SectionTitle>
        <Field label="Repository URL">
          <input className={`${inputCls} font-mono`} value={repo} onChange={(e) => setRepo(e.target.value)} />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Branch">
            <input className={`${inputCls} font-mono`} value={branch} onChange={(e) => setBranch(e.target.value)} />
          </Field>
          <Field label="Commit SHA (HEAD = latest)">
            <input className={`${inputCls} font-mono`} value={commitSha} onChange={(e) => setCommitSha(e.target.value)} />
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Base directory">
            <input className={`${inputCls} font-mono`} value={baseDirectory} onChange={(e) => setBaseDirectory(e.target.value)} />
          </Field>
          <Field label="Publish directory">
            <input
              className={`${inputCls} font-mono`}
              value={publishDirectory}
              onChange={(e) => setPublishDirectory(e.target.value)}
              placeholder="dist"
            />
          </Field>
        </div>
      </div>

      <div className={`${cardCls} flex flex-col gap-4`}>
        <SectionTitle>Build commands (override auto-detection)</SectionTitle>
        <Field label="Install command">
          <input
            className={`${inputCls} font-mono`}
            value={installCommand}
            onChange={(e) => setInstallCommand(e.target.value)}
            placeholder="npm ci"
          />
        </Field>
        <Field label="Build command">
          <input
            className={`${inputCls} font-mono`}
            value={buildCommand}
            onChange={(e) => setBuildCommand(e.target.value)}
            placeholder="npm run build"
          />
        </Field>
        <Field label="Start command">
          <input
            className={`${inputCls} font-mono`}
            value={startCommand}
            onChange={(e) => setStartCommand(e.target.value)}
            placeholder="npm start"
          />
        </Field>
      </div>

      <div className={`${cardCls} flex flex-col gap-4`}>
        <SectionTitle>Build pack specifics</SectionTitle>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Dockerfile location">
            <input
              className={`${inputCls} font-mono`}
              value={dockerfileLocation}
              onChange={(e) => setDockerfileLocation(e.target.value)}
            />
          </Field>
          <Field label="Compose file location">
            <input
              className={`${inputCls} font-mono`}
              value={composeLocation}
              onChange={(e) => setComposeLocation(e.target.value)}
            />
          </Field>
        </div>
        <Field label="Static image (static build pack)">
          <input className={`${inputCls} font-mono`} value={staticImage} onChange={(e) => setStaticImage(e.target.value)} />
        </Field>
      </div>

      <ErrorNote error={save.error} />
      <button type="submit" className={`${btnPrimary} w-fit`} disabled={save.isPending}>
        {save.isPending ? 'Saving…' : 'Save'}
      </button>
    </form>
  )
}
