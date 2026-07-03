import { ScheduledTasks } from '../../../components/ScheduledTasks'
import { useApplication } from './index'

/** Scheduled-tasks tab on the application detail page. */
export default function ApplicationTasks() {
  const { app } = useApplication()
  return <ScheduledTasks resource="applications" uuid={app.uuid} />
}
