<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { shallowRef } from 'vue'

interface EngineReport {
  gitVersion: string
  binaryPath: string
  refFormat: string
  checks: { name: string; detail: string }[]
  objectBytesBefore: number
  objectBytesAfter: number
}

type Verification =
  | { status: 'idle' }
  | { status: 'running' }
  | { status: 'passed'; report: EngineReport }
  | { status: 'failed'; error: string }

// 此状态对应原生窗口中的一次引擎验收，结果只来自 Rust 命令。
const verification = shallowRef<Verification>({ status: 'idle' })

const formatBytes = new Intl.NumberFormat('zh-CN', {
  style: 'unit',
  unit: 'byte',
  unitDisplay: 'short',
})

async function runVerification() {
  verification.value = { status: 'running' }
  try {
    const report = await invoke<EngineReport>('verify_git_engine')
    verification.value = { status: 'passed', report }
  } catch (error) {
    verification.value = {
      status: 'failed',
      error: error instanceof Error ? error.message : String(error),
    }
  }
}
</script>

<template>
  <main class="workspace">
    <header class="topbar">
      <span class="product">FileManagementMaster</span>
      <span
        class="status"
        :data-status="verification.status"
        role="status"
      >{{ { idle: '待验证', running: '验证中', passed: '已通过', failed: '验证失败' }[verification.status] }}</span>
    </header>

    <section class="introduction" aria-labelledby="title">
      <div>
        <h1 id="title">Git 引擎验证</h1>
        <p>在临时书库中检查随包 Git、分支引用和实际对象回收。</p>
      </div>
      <button type="button" :disabled="verification.status === 'running'" @click="runVerification">
        {{ verification.status === 'running' ? '正在验证' : '运行验证' }}
      </button>
    </section>

    <section v-if="verification.status === 'passed'" class="results" aria-labelledby="results-title">
      <div class="result-heading">
        <div>
          <h2 id="results-title">验证结果</h2>
          <p class="technical">{{ verification.report.gitVersion }} · {{ verification.report.refFormat }}</p>
        </div>
        <div class="reclaimed">
          <span>对象库缩减</span>
          <strong>{{ formatBytes.format(verification.report.objectBytesBefore - verification.report.objectBytesAfter) }}</strong>
        </div>
      </div>
      <dl class="checks">
        <div v-for="check in verification.report.checks" :key="check.name" class="check">
          <dt>{{ check.name }}</dt>
          <dd>{{ check.detail }}</dd>
        </div>
      </dl>
      <p class="binary"><span>随包程序</span><code>{{ verification.report.binaryPath }}</code></p>
    </section>

    <section v-else-if="verification.status === 'failed'" class="results error" role="alert">
      <h2>验证失败</h2>
      <p>{{ verification.error }}</p>
    </section>
    <section v-else class="results empty">
      <h2>{{ verification.status === 'running' ? '正在检查临时书库' : '验证项目' }}</h2>
      <p>随包运行时 · 引用事务 · 中断后的索引恢复 · 过期对象回收</p>
    </section>
  </main>
</template>
