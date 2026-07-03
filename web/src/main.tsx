import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { createBrowserRouter, RouterProvider } from 'react-router'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import './index.css'
import { Layout } from './components/Layout'
import Login from './routes/login'
import Onboarding from './routes/onboarding'
import Dashboard from './routes/dashboard'
import ServerPage from './routes/servers/[uuid]'
import ProjectPage from './routes/projects/[uuid]'
import ApplicationGeneral, { ApplicationLayout } from './routes/applications/[uuid]/index'
import ApplicationEnvs from './routes/applications/[uuid]/envs'
import ApplicationStorage from './routes/applications/[uuid]/storage'
import ApplicationSource from './routes/applications/[uuid]/source'
import ApplicationDomains from './routes/applications/[uuid]/domains'
import ApplicationDeployments from './routes/applications/[uuid]/deployments'
import ApplicationPreviews from './routes/applications/[uuid]/previews'
import ApplicationTasks from './routes/applications/[uuid]/tasks'
import DeploymentPage from './routes/deployments/[uuid]'
import DatabasesList from './routes/databases/index'
import NewDatabase from './routes/databases/new'
import DatabasePage from './routes/databases/[uuid]'
import ServicesCatalog from './routes/services/index'
import ServicePage from './routes/services/[uuid]'
import Settings from './routes/settings'
import SourcesPage from './routes/sources/index'
import SourceDetailPage from './routes/sources/[uuid]'
import NotificationsPage from './routes/notifications'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
})

const router = createBrowserRouter([
  { path: '/login', element: <Login /> },
  {
    element: <Layout />,
    children: [
      { path: '/', element: <Dashboard /> },
      { path: '/onboarding', element: <Onboarding /> },
      { path: '/servers/:uuid', element: <ServerPage /> },
      { path: '/projects/:uuid', element: <ProjectPage /> },
      {
        path: '/applications/:uuid',
        element: <ApplicationLayout />,
        children: [
          { index: true, element: <ApplicationGeneral /> },
          { path: 'envs', element: <ApplicationEnvs /> },
          { path: 'storage', element: <ApplicationStorage /> },
          { path: 'source', element: <ApplicationSource /> },
          { path: 'domains', element: <ApplicationDomains /> },
          { path: 'deployments', element: <ApplicationDeployments /> },
          { path: 'previews', element: <ApplicationPreviews /> },
          { path: 'tasks', element: <ApplicationTasks /> },
        ],
      },
      { path: '/deployments/:uuid', element: <DeploymentPage /> },
      { path: '/databases', element: <DatabasesList /> },
      { path: '/databases/new', element: <NewDatabase /> },
      { path: '/databases/:uuid', element: <DatabasePage /> },
      { path: '/services', element: <ServicesCatalog /> },
      { path: '/services/:uuid', element: <ServicePage /> },
      { path: '/sources', element: <SourcesPage /> },
      { path: '/sources/github/:uuid', element: <SourceDetailPage /> },
      { path: '/notifications', element: <NotificationsPage /> },
      { path: '/settings', element: <Settings /> },
    ],
  },
])

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
