use num_traits::{One, Zero};
use std::convert::TryInto;

use twenty_first::shared_math::b_field_element::BFieldElement;
use twenty_first::shared_math::mpolynomial::MPolynomial;
use twenty_first::shared_math::other;
use twenty_first::shared_math::x_field_element::XFieldElement;

use crate::lift_coefficients_to_xfield;
use crate::stark::{EXTENSION_CHALLENGE_COUNT, PERMUTATION_ARGUMENTS_COUNT, TERMINAL_COUNT};
use crate::table::{Table, TableMoreTrait, TableTrait};
use crate::vm::Register;

#[derive(Debug, Clone)]
pub struct MemoryTable(pub Table<MemoryTableMore>);

#[derive(Debug, Clone)]
pub struct MemoryTableMore {
    pub permutation_terminal: XFieldElement,
}

impl TableMoreTrait for MemoryTableMore {
    fn new_more() -> Self {
        MemoryTableMore {
            permutation_terminal: XFieldElement::zero(),
        }
    }
}

impl MemoryTable {
    // named indices for base columns
    pub const CYCLE: usize = 0;
    pub const MEMORY_POINTER: usize = 1;
    pub const MEMORY_VALUE: usize = 2;
    pub const INTERWEAVED: usize = 3;

    // named indices for extension columns
    pub const PERMUTATION: usize = 4;

    // base and extension table width
    pub const BASE_WIDTH: usize = 4;
    pub const FULL_WIDTH: usize = 5;

    pub fn new(
        length: usize,
        num_randomizers: usize,
        generator: BFieldElement,
        order: usize,
    ) -> Self {
        let table = Table::<MemoryTableMore>::new(
            Self::BASE_WIDTH,
            Self::FULL_WIDTH,
            length,
            num_randomizers,
            generator,
            order,
            "Memory table".to_string(),
        );

        Self(table)
    }

    pub fn pad(&mut self) {
        while !other::is_power_of_two(self.0.matrix.len()) {
            let mut padding = self.0.matrix.last().unwrap().to_owned();
            padding[Self::CYCLE] += BFieldElement::one();
            padding[Self::INTERWEAVED] = BFieldElement::one();
            self.0.matrix.push(padding);
        }
    }

    pub fn derive_matrix(processor_matrix: &[Register]) -> Vec<Vec<BFieldElement>> {
        let mut matrix = vec![];
        for (i, pt) in processor_matrix.iter().enumerate() {
            // This means that the memory value after the last instruction execution is not constrained.
            // But it can't be output, so that's OK, I guess.
            if i == processor_matrix.len() - 1 {
                continue;
            } else {
                assert!(
                    !pt.current_instruction.is_zero(),
                    "Processor matrix must be unpadded when deriving memory matrix. Row {i} has instruction zero. Input was: {processor_matrix:?}" 
                );
            }
            matrix.push(vec![
                pt.cycle,
                pt.memory_pointer,
                pt.memory_value,
                BFieldElement::zero(),
            ]);
        }

        matrix.sort_by_key(|k| k[MemoryTable::MEMORY_POINTER].value());

        // Interweave rows to ensure that clock cycle increases by one per row
        // All rows that are not present in the processor table are interweaved rows
        let one = BFieldElement::one();
        let interweave_indicator = one;
        let mut i = 1;
        while i < matrix.len() - 1 {
            if matrix[i + 1][Self::MEMORY_POINTER] == matrix[i][Self::MEMORY_POINTER]
                && matrix[i + 1][Self::CYCLE] != matrix[i][Self::CYCLE] + one
            {
                let interleaved_value: Vec<BFieldElement> = vec![
                    matrix[i][Self::CYCLE] + one,
                    matrix[i][Self::MEMORY_POINTER],
                    matrix[i][Self::MEMORY_VALUE],
                    interweave_indicator,
                ];
                matrix.insert(i + 1, interleaved_value);
            }
            i += 1;
        }

        matrix
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_constraints_afo_named_variables(
        cycle: MPolynomial<BFieldElement>,
        address: MPolynomial<BFieldElement>,
        value: MPolynomial<BFieldElement>,
        interweaved: MPolynomial<BFieldElement>,
        cycle_next: MPolynomial<BFieldElement>,
        address_next: MPolynomial<BFieldElement>,
        value_next: MPolynomial<BFieldElement>,
        interweaved_next: MPolynomial<BFieldElement>,
    ) -> Vec<MPolynomial<BFieldElement>> {
        let mut polynomials: Vec<MPolynomial<BFieldElement>> = vec![];

        let variable_count = Self::BASE_WIDTH * 2;
        let one = MPolynomial::from_constant(BFieldElement::one(), variable_count);

        // 1. memory pointer increases by one or zero
        // <=>. (MP*=MP+1) \/ (MP*=MP)
        polynomials.push(
            (address_next.clone() - address.clone() - one.clone())
                * (address_next.clone() - address.clone()),
        );

        // 2. If memory pointer does not increase, the clock cycle must increase by one
        polynomials.push(
            (address_next.clone() - address.clone() - one.clone())
                * (cycle_next - cycle - one.clone()),
        );

        // If row is an interweaved row, the clock cycle must increase by one (covered by 2 and 3)

        // 3. If row is an interweaved row, the memory pointer may not change
        polynomials.push(interweaved.clone() * (address_next.clone() - address.clone()));

        // 4. If row is an interweaved row, the memory value may not change
        polynomials.push(interweaved * (value - value_next.clone()));

        // 5. Interweave value is either one or zero. We have to check the next value
        // as the last row is not otherwise checked. I don't think we have to check a
        // boundary condition as the other rules for `interweaved` guarantee that
        // this is 0 in the 1st row.
        polynomials.push(interweaved_next.clone() * (interweaved_next - one));

        // 6. if memory pointer increases by one, then memory value must be set to zero
        polynomials.push((address_next - address) * value_next);

        polynomials
    }
}

impl TableTrait for MemoryTable {
    fn base_width(&self) -> usize {
        self.0.base_width
    }

    fn full_width(&self) -> usize {
        self.0.full_width
    }

    fn length(&self) -> usize {
        self.0.length
    }

    fn name(&self) -> &str {
        &self.0.name
    }

    fn num_randomizers(&self) -> usize {
        self.0.num_randomizers
    }

    fn height(&self) -> usize {
        self.0.height
    }

    fn omicron(&self) -> BFieldElement {
        self.0.omicron
    }

    fn generator(&self) -> BFieldElement {
        self.0.generator
    }

    fn order(&self) -> usize {
        self.0.order
    }

    fn base_transition_constraints(&self) -> Vec<MPolynomial<BFieldElement>> {
        let variable_count = Self::BASE_WIDTH * 2;
        let vars = MPolynomial::<BFieldElement>::variables(variable_count);

        let cycle = vars[0].clone();
        let address = vars[1].clone();
        let value = vars[2].clone();
        let interweaved = vars[3].clone();
        let cycle_next = vars[4].clone();
        let address_next = vars[5].clone();
        let value_next = vars[6].clone();
        let interweaved_next = vars[7].clone();

        MemoryTable::transition_constraints_afo_named_variables(
            cycle,
            address,
            value,
            interweaved,
            cycle_next,
            address_next,
            value_next,
            interweaved_next,
        )
    }

    fn base_boundary_constraints(&self) -> Vec<MPolynomial<BFieldElement>> {
        todo!()
    }

    fn extend(
        &mut self,
        all_challenges: [XFieldElement; EXTENSION_CHALLENGE_COUNT],
        all_initials: [XFieldElement; PERMUTATION_ARGUMENTS_COUNT],
    ) {
        let _a = all_challenges[0];
        let _b = all_challenges[1];
        let _c = all_challenges[2];
        let d = all_challenges[3];
        let e = all_challenges[4];
        let f = all_challenges[5];
        let _alpha = all_challenges[6];
        let beta = all_challenges[7];
        let _gamma = all_challenges[8];
        let _delta = all_challenges[9];
        let _eta = all_challenges[10];

        let _processor_instruction_permutation_initial = all_initials[0];
        let processor_memory_permutation_initial = all_initials[1];

        // prepare loop
        let mut extended_matrix: Vec<Vec<XFieldElement>> =
            vec![Vec::with_capacity(self.full_width()); self.0.matrix.len()];
        let mut memory_permutation_running_product = processor_memory_permutation_initial;

        // loop over all rows of table
        for (i, row) in self.0.matrix.iter().enumerate() {
            let mut new_row: Vec<XFieldElement> = row.iter().map(|bfe| bfe.lift()).collect();

            new_row.push(memory_permutation_running_product);

            if new_row[Self::INTERWEAVED].is_zero() {
                memory_permutation_running_product *= beta
                    - d * new_row[MemoryTable::CYCLE]
                    - e * new_row[MemoryTable::MEMORY_POINTER]
                    - f * new_row[MemoryTable::MEMORY_VALUE];
            }
            extended_matrix[i] = new_row;
        }

        self.0.extended_matrix = extended_matrix;

        self.0.extended_codewords = self
            .0
            .codewords
            .iter()
            .map(|row| row.iter().map(|elem| elem.lift()).collect())
            .collect();

        self.0.more.permutation_terminal = memory_permutation_running_product;
    }

    fn transition_constraints_ext(
        &self,
        challenges: [XFieldElement; EXTENSION_CHALLENGE_COUNT],
    ) -> Vec<MPolynomial<XFieldElement>> {
        let [_a, _b, _c, d, e, f, _alpha, beta, _gamma, _delta, _eta]: [MPolynomial<XFieldElement>;
            EXTENSION_CHALLENGE_COUNT] = challenges
            .iter()
            .map(|challenge| MPolynomial::from_constant(*challenge, 2 * Self::FULL_WIDTH))
            .collect::<Vec<MPolynomial<XFieldElement>>>()
            .try_into()
            .unwrap();

        let b_field_variables: [MPolynomial<BFieldElement>; 2 * Self::FULL_WIDTH] =
            MPolynomial::variables(2 * Self::FULL_WIDTH)
                .try_into()
                .unwrap();
        let [b_field_cycle, b_field_address, b_field_value, b_field_interweaved, _b_field_permutation, b_field_cycle_next, b_field_address_next, b_field_value_next, b_field_interweaved_next, _b_field_permutation_next] =
            b_field_variables;

        let b_field_polynomials = Self::transition_constraints_afo_named_variables(
            b_field_cycle,
            b_field_address,
            b_field_value,
            b_field_interweaved,
            b_field_cycle_next,
            b_field_address_next,
            b_field_value_next,
            b_field_interweaved_next,
        );

        let b_field_polylen = b_field_polynomials.len();
        assert_eq!(
            6, b_field_polylen,
            "number of transition constraints from MemoryTable is {b_field_polylen}, but expected 6"
        );

        let x_field_variables: [MPolynomial<XFieldElement>; 2 * Self::FULL_WIDTH] =
            MPolynomial::variables(2 * Self::FULL_WIDTH)
                .try_into()
                .unwrap();
        let [cycle, address, value, interweaved, permutation, _cycle_next, _address_next, _value_next, _interweaved_next, permutation_next] =
            x_field_variables;

        let mut polynomials: Vec<MPolynomial<XFieldElement>> = b_field_polynomials
            .iter()
            .map(lift_coefficients_to_xfield)
            .collect();

        let one: MPolynomial<XFieldElement> =
            MPolynomial::from_constant(XFieldElement::one(), 2 * Self::FULL_WIDTH);
        polynomials.push(
            permutation
                * ((beta - d * cycle - e * address - f * value) * (one - interweaved.clone())
                    + interweaved)
                - permutation_next,
        );

        polynomials
    }

    fn boundary_constraints_ext(
        &self,
        // TODO: Is `challenges` really not needed here?
        _challenges: [XFieldElement; EXTENSION_CHALLENGE_COUNT],
    ) -> Vec<MPolynomial<XFieldElement>> {
        let zero = MPolynomial::<XFieldElement>::zero(Self::FULL_WIDTH);
        let x = MPolynomial::<XFieldElement>::variables(Self::FULL_WIDTH);

        let cycle = x[MemoryTable::CYCLE].clone();
        let memory_pointer = x[MemoryTable::MEMORY_POINTER].clone();
        let memory_value = x[MemoryTable::MEMORY_VALUE].clone();

        vec![
            cycle - zero.clone(),
            memory_pointer - zero.clone(),
            memory_value - zero,
            // I think we don't have to enforce that the `INTERWEAVE` value is zero
            // in row 0 since any table where that's not the case will fail its
            // permutation check with the processor table
        ]
    }

    fn terminal_constraints_ext(
        &self,
        challenges: [XFieldElement; EXTENSION_CHALLENGE_COUNT],
        terminals: [XFieldElement; TERMINAL_COUNT],
    ) -> Vec<MPolynomial<XFieldElement>> {
        let [_a, _b, _c, d, e, f, _alpha, beta, _gamma, _delta, _eta]: [MPolynomial<XFieldElement>;
            EXTENSION_CHALLENGE_COUNT] = challenges
            .iter()
            .map(|challenge| MPolynomial::from_constant(*challenge, Self::FULL_WIDTH))
            .collect::<Vec<MPolynomial<XFieldElement>>>()
            .try_into()
            .unwrap();

        let processor_memory_permutation_terminal =
            MPolynomial::<XFieldElement>::from_constant(terminals[1], Self::FULL_WIDTH);

        let x = MPolynomial::<XFieldElement>::variables(Self::FULL_WIDTH);

        let cycle = x[MemoryTable::CYCLE].clone();
        let memory_pointer = x[MemoryTable::MEMORY_POINTER].clone();
        let memory_value = x[MemoryTable::MEMORY_VALUE].clone();
        let permutation = x[Self::PERMUTATION].clone();
        let interweaved = x[Self::INTERWEAVED].clone();
        let one =
            MPolynomial::<XFieldElement>::from_constant(XFieldElement::one(), Self::FULL_WIDTH);

        vec![
            (permutation.clone() * (beta - d * cycle - e * memory_pointer - f * memory_value)
                - processor_memory_permutation_terminal.clone())
                * (one - interweaved.clone())
                + (permutation - processor_memory_permutation_terminal) * interweaved,
        ]
    }
}

#[cfg(test)]
mod memory_table_tests {
    use super::*;

    use twenty_first::shared_math::other::random_elements_array;
    use twenty_first::shared_math::traits::PrimitiveRootOfUnity;

    use crate as brainfuck;
    use crate::vm::sample_programs;
    use crate::vm::BaseMatrices;

    // When we simulate a program, this generates a collection of matrices that contain
    // "abstract" execution traces. When we evaluate the base transition constraints on
    // the rows (points) from the InstructionTable matrix, these should evaluate to zero.
    #[test]
    fn memory_base_table_evaluate_to_zero_on_execution_trace_test() {
        for source_code in sample_programs::get_all_sample_programs().iter() {
            let actual_program = brainfuck::vm::compile(source_code).unwrap();
            let input_data = vec![
                BFieldElement::new(97),
                BFieldElement::new(98),
                BFieldElement::new(99),
            ];
            let base_matrices: BaseMatrices =
                brainfuck::vm::simulate(&actual_program, &input_data).unwrap();
            let derived_memory_matrix = MemoryTable::derive_matrix(&base_matrices.processor_matrix);

            assert!(
                !derived_memory_matrix.is_empty(),
                "All tested programs update memory"
            );

            let number_of_randomizers = 2;
            let order = 1 << 32;
            let smooth_generator = BFieldElement::primitive_root_of_unity(order).unwrap();

            // instantiate table objects
            let mut memory_table: MemoryTable = MemoryTable::new(
                derived_memory_matrix.len(),
                number_of_randomizers,
                smooth_generator,
                order as usize,
            );

            let air_constraints = memory_table.base_transition_constraints();

            let step_count = std::cmp::max(0, derived_memory_matrix.len() as isize - 1) as usize;
            for step in 0..step_count {
                let row: Vec<BFieldElement> = derived_memory_matrix[step].clone();
                let next_row: Vec<BFieldElement> = derived_memory_matrix[step + 1].clone();
                let point: Vec<BFieldElement> = vec![row, next_row].concat();

                for air_constraint in air_constraints.iter() {
                    assert!(air_constraint.evaluate(&point).is_zero());
                }
            }

            // Test transition constraints on extension table
            let challenges: [XFieldElement; EXTENSION_CHALLENGE_COUNT] = random_elements_array();
            let initials: [XFieldElement; PERMUTATION_ARGUMENTS_COUNT] = random_elements_array();
            memory_table.extend(challenges, initials);

            // Get transition constraints for extension table instead
            let mem_air_constraints_ext = memory_table.transition_constraints_ext(challenges);

            let extended_matrix_len = memory_table.0.extended_matrix.len() as isize;
            let extended_steps = std::cmp::max(0, extended_matrix_len - 1) as usize;
            for step in 0..extended_steps {
                let row = memory_table.0.extended_matrix[step].clone();
                let next_row = memory_table.0.extended_matrix[step + 1].clone();
                let xpoint: Vec<XFieldElement> = vec![row, next_row].concat();

                for air_constraint_ext in mem_air_constraints_ext.iter() {
                    assert!(air_constraint_ext.evaluate(&xpoint).is_zero());
                }
            }
        }
    }
}
