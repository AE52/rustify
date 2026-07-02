import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router'
import { api, type User } from '../api/client'
import { btnPrimary, ErrorNote, Field, inputCls } from '../components/ui'

export default function Login() {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const login = useMutation({
    mutationFn: () => api.post<{ user: User }>('/auth/login', { email, password }),
    onSuccess: () => {
      queryClient.clear()
      navigate('/', { replace: true })
    },
  })

  const submit = (e: FormEvent) => {
    e.preventDefault()
    if (!login.isPending) login.mutate()
  }

  return (
    <div className="grid min-h-screen place-items-center px-4">
      <div className="w-full max-w-sm">
        <h1 className="mb-1 text-center text-2xl font-bold tracking-tight text-zinc-100">rustify</h1>
        <p className="mb-8 text-center text-sm text-zinc-500">Sign in to your instance</p>
        <form onSubmit={submit} className="flex flex-col gap-4 rounded-lg border border-zinc-800 bg-zinc-900/40 p-6">
          <Field label="Email">
            <input
              type="email"
              required
              autoComplete="email"
              className={inputCls}
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </Field>
          <Field label="Password">
            <input
              type="password"
              required
              autoComplete="current-password"
              className={inputCls}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </Field>
          <ErrorNote error={login.error} />
          <button type="submit" className={btnPrimary} disabled={login.isPending}>
            {login.isPending ? 'Signing in…' : 'Sign in'}
          </button>
        </form>
      </div>
    </div>
  )
}
