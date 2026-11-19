import { toast } from 'vue-sonner'

export function messageText(cause: unknown): string {
  return (cause instanceof Error ? cause.message : String(cause ?? '')).trim() || '操作失败'
}

export const Message = {
  error(cause: unknown, id?: string) {
    const text = messageText(cause)
    toast.error(text, { id: id ?? `error:${text}`, duration: 4500 })
  },
  success(text: string) {
    toast.success(text, { id: `success:${text}`, duration: 2600 })
  },
  dismiss(id: string) {
    toast.dismiss(id)
  },
}
