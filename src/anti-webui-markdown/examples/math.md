# Formulas

Inline formulas sit on the baseline of the text: the energy $E = mc^2$, the golden ratio
$\varphi = \frac{1 + \sqrt{5}}{2}$, a sum $\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$, and
$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$ in the middle of a sentence. 日本語の文の中でも
$a^2 + b^2 = c^2$ のように並びます。

A formula on a line of its own:

$$
f(x) = \sum_{n=0}^{\infty} \frac{f^{(n)}(a)}{n!} (x - a)^n
$$

Text before $$\oint_C \mathbf{F} \cdot d\mathbf{r} = \iint_S (\nabla \times \mathbf{F}) \cdot d\mathbf{S}$$ and after.

```math
\begin{aligned}
(a + b)^2 &= a^2 + 2ab + b^2 \\
          &\le 2(a^2 + b^2)
\end{aligned}
```

$$
\tag{1} y = mx + b
$$

## Numbers and references

$$
\begin{equation}
E = mc^2 \label{eq:energy}
\end{equation}
$$

$$
\begin{align}
\nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \label{eq:gauss} \\
\nabla \cdot \mathbf{B} &= 0 \nonumber \\
\nabla \times \mathbf{E} &= -\frac{\partial \mathbf{B}}{\partial t} \label{eq:faraday}
\end{align}
$$

Equations are numbered through the document: $\eqref{eq:energy}$ is the first, Gauss's law
$\eqref{eq:gauss}$ the second, and Faraday's $\ref{eq:faraday}$ the third. Clicking one goes to it.

A long formula among words breaks after its relations and operators, as KaTeX's does:
$f(x) = a_0 + a_1 x + a_2 x^2 + a_3 x^3 + a_4 x^4 + a_5 x^5 + a_6 x^6 + a_7 x^7 + a_8 x^8 + a_9 x^9 + a_{10} x^{10} + a_{11} x^{11} = \sum_{k=0}^{11} a_k x^k$
and the text goes on after it.

## Matrices and cases

$$
A = \begin{pmatrix} a_{11} & \cdots & a_{1n} \\ \vdots & \ddots & \vdots \\ a_{m1} & \cdots & a_{mn} \end{pmatrix},
\qquad
f(x) = \begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}
$$

| Name | Formula |
|---|---|
| Euler | $e^{i\pi} + 1 = 0$ |
| Bayes | $P(A \mid B) = \frac{P(B \mid A)\,P(A)}{P(B)}$ |
| Chemistry | $\ce{2H2 + O2 -> 2H2O}$ |

- In a list: $\lim_{x \to 0} \frac{\sin x}{x} = 1$
- Colors: $\color{red} a + \color{blue} b$, and $\boxed{x}$
- What can't be read shows as written: $\frac{1}{x$

> A quote with $\mathbb{R}^n$ and $\mathcal{L}(\theta)$.

A very wide one scrolls sideways:

$$
\sum_{k=1}^{n} \left( a_k + b_k + c_k + d_k + e_k + f_k + g_k + h_k + i_k + j_k + k_k + l_k + m_k + n_k + o_k + p_k + q_k + r_k + s_k + t_k + u_k + v_k + w_k + x_k + y_k + z_k \right)
$$
